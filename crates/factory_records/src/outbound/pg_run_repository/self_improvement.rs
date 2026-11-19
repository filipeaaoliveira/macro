//! The Postgres Run repository's self-improvement records
//! ([`SelfImprovementRepository`]): `factory_improvement_failure`,
//! `factory_improvement`, and who named a team's Definition.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::models::{RunId, RunOutcome};
use crate::domain::self_improvement::{
    DefinitionNaming, Failure, FailureSource, Improvement, ImprovementBegin, ImprovementState,
    MAX_FAILURE_REASON_BYTES, NewFailure, PendingFailures, RunFailureFacts,
    SelfImprovementRepository, Settlement,
};

/// The kind column of a Scorer's failure.
const SCORER_KIND: &str = "scorer";

/// The kind column of a flawed Run's failure.
const FLAWED_KIND: &str = "flawed";

/// How many sources a team's pending failures are counted by at most.
const SOURCES_READ: i64 = 50;

/// A count the schema keeps far below `u32::MAX`.
fn count(stored: i64) -> u32 {
    u32::try_from(stored).unwrap_or(u32::MAX)
}

/// `text` cut to at most `max` bytes, on a character's boundary.
fn cut_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A failure's source as its row keeps it.
fn source(kind: &str, scorer: String, score: Option<f64>) -> FailureSource {
    match (kind, score) {
        (SCORER_KIND, Some(score)) => FailureSource::Scorer {
            name: scorer,
            score,
        },
        _ => FailureSource::Flawed,
    }
}

/// An improvement as its row keeps it.
#[expect(clippy::too_many_arguments, reason = "one per column")]
fn improvement(
    id: Uuid,
    team_id: Uuid,
    repo_url: String,
    run_id: Option<Uuid>,
    state: String,
    refusal: Option<String>,
    created_at: DateTime<Utc>,
    settled_at: Option<DateTime<Utc>>,
    failures: i32,
) -> Result<Improvement, PgRunRepositoryErr> {
    let state = state
        .parse::<ImprovementState>()
        .map_err(|_| PgRunRepositoryErr::UnknownImprovementState(state))?;
    Ok(Improvement {
        id,
        team_id,
        repo_url,
        run_id: run_id.map(RunId::from_uuid),
        state,
        refusal,
        created_at,
        settled_at,
        failures: u32::try_from(failures).unwrap_or_default(),
    })
}

impl SelfImprovementRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn definition_naming(
        &self,
        team_id: Uuid,
    ) -> Result<Option<DefinitionNaming>, Self::Err> {
        let row = sqlx::query!(
            "SELECT repo_url, named_by_user_id FROM factory_team_definition WHERE team_id = $1",
            team_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let named_by = row
            .named_by_user_id
            .map(|id| {
                MacroUserIdStr::try_from(id.clone())
                    .map_err(|_| PgRunRepositoryErr::InvalidUserId(id))
            })
            .transpose()?;
        Ok(Some(DefinitionNaming {
            repo_url: row.repo_url,
            named_by,
        }))
    }

    #[tracing::instrument(err, skip(self))]
    async fn failure_facts(&self, run_id: RunId) -> Result<Option<RunFailureFacts>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT user_id, flow, repo_url, definition_repo_url, definition_evaluation, outcome,
                flaw_reason
            FROM factory_run
            WHERE id = $1
            "#,
            run_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let judgments = sqlx::query!(
            "SELECT scorer, score, reason FROM factory_run_score WHERE run_id = $1 ORDER BY scorer",
            run_id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|judgment| (judgment.scorer, judgment.score, judgment.reason))
        .collect();
        let outcome = row
            .outcome
            .map(|outcome| {
                outcome
                    .parse::<RunOutcome>()
                    .map_err(|_| PgRunRepositoryErr::UnknownRunOutcome(outcome))
            })
            .transpose()?;
        Ok(Some(RunFailureFacts {
            user_id: MacroUserIdStr::try_from(row.user_id.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.user_id))?,
            flow: row.flow.unwrap_or_default(),
            repo_url: row.repo_url,
            definition_repo_url: row.definition_repo_url,
            definition_evaluation: row.definition_evaluation,
            outcome,
            flaw_reason: row.flaw_reason,
            judgments,
        }))
    }

    #[tracing::instrument(err, skip(self, failures))]
    async fn record_failures(
        &self,
        team_id: Uuid,
        repo_url: &str,
        run_id: RunId,
        failures: &[NewFailure],
    ) -> Result<(), Self::Err> {
        let mut tx = self.pool.begin().await?;
        for failure in failures {
            let (kind, scorer, score) = match &failure.source {
                FailureSource::Scorer { name, score } => (SCORER_KIND, name.as_str(), Some(*score)),
                FailureSource::Flawed => (FLAWED_KIND, "", None),
            };
            sqlx::query!(
                r#"
                INSERT INTO factory_improvement_failure
                    (run_id, kind, scorer, team_id, repo_url, score, reason)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                ON CONFLICT (run_id, kind, scorer) DO NOTHING
                "#,
                run_id.as_uuid(),
                kind,
                scorer,
                team_id,
                repo_url,
                score,
                cut_bytes(&failure.reason, MAX_FAILURE_REASON_BYTES),
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn pending_failures(
        &self,
        team_id: Uuid,
        repo_url: &str,
    ) -> Result<PendingFailures, Self::Err> {
        let sources = sqlx::query!(
            r#"
            SELECT CASE WHEN kind = 'flawed' THEN 'flawed' ELSE scorer END AS "label!",
                count(*) AS "count!", min(failed_at) AS "oldest!"
            FROM factory_improvement_failure
            WHERE team_id = $1 AND improvement_id IS NULL AND lower(repo_url) = lower($2)
            GROUP BY 1
            ORDER BY 2 DESC, 1
            LIMIT $3
            "#,
            team_id,
            repo_url,
            SOURCES_READ,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut pending = PendingFailures::default();
        for source in sources {
            pending.count = pending.count.saturating_add(count(source.count));
            pending.oldest = Some(match pending.oldest {
                Some(oldest) => oldest.min(source.oldest),
                None => source.oldest,
            });
            pending.by_source.push((source.label, count(source.count)));
        }
        Ok(pending)
    }

    #[tracing::instrument(err, skip(self))]
    async fn latest_improvement(&self, team_id: Uuid) -> Result<Option<Improvement>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT id, team_id, repo_url, run_id, state, refusal, created_at, settled_at, failures
            FROM factory_improvement
            WHERE team_id = $1
            ORDER BY created_at DESC, id DESC
            LIMIT 1
            "#,
            team_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            improvement(
                row.id,
                row.team_id,
                row.repo_url,
                row.run_id,
                row.state,
                row.refusal,
                row.created_at,
                row.settled_at,
                row.failures,
            )
        })
        .transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn recent_improvements(
        &self,
        team_id: Uuid,
        limit: u32,
    ) -> Result<Vec<Improvement>, Self::Err> {
        let rows = sqlx::query!(
            r#"
            SELECT id, team_id, repo_url, run_id, state, refusal, created_at, settled_at, failures
            FROM factory_improvement
            WHERE team_id = $1
            ORDER BY created_at DESC, id DESC
            LIMIT $2
            "#,
            team_id,
            i64::from(limit),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                improvement(
                    row.id,
                    row.team_id,
                    row.repo_url,
                    row.run_id,
                    row.state,
                    row.refusal,
                    row.created_at,
                    row.settled_at,
                    row.failures,
                )
            })
            .collect()
    }

    #[tracing::instrument(err, skip(self))]
    async fn begin_improvement(
        &self,
        id: Uuid,
        team_id: Uuid,
        repo_url: &str,
        limit: u32,
    ) -> Result<ImprovementBegin, Self::Err> {
        let mut tx = self.pool.begin().await?;
        // The open index keeps one starting or open Run per team: a second
        // begin at once keeps nothing.
        let kept = sqlx::query_scalar!(
            r#"
            INSERT INTO factory_improvement (id, team_id, repo_url, state)
            VALUES ($1, $2, $3, 'starting')
            ON CONFLICT (team_id) WHERE state IN ('starting', 'open') DO NOTHING
            RETURNING id
            "#,
            id,
            team_id,
            repo_url,
        )
        .fetch_optional(&mut *tx)
        .await?;
        if kept.is_none() {
            return Ok(ImprovementBegin::AlreadyOpen);
        }
        let claimed = sqlx::query!(
            r#"
            UPDATE factory_improvement_failure f
            SET improvement_id = $1
            FROM (
                SELECT run_id, kind, scorer
                FROM factory_improvement_failure
                WHERE team_id = $2 AND improvement_id IS NULL AND lower(repo_url) = lower($3)
                ORDER BY failed_at, run_id, kind, scorer
                LIMIT $4
                FOR UPDATE
            ) pending
            WHERE f.run_id = pending.run_id AND f.kind = pending.kind
                AND f.scorer = pending.scorer
            RETURNING f.run_id
            "#,
            id,
            team_id,
            repo_url,
            i64::from(limit),
        )
        .fetch_all(&mut *tx)
        .await?;
        if claimed.is_empty() {
            return Ok(ImprovementBegin::NoFailures);
        }
        let held = i32::try_from(claimed.len()).unwrap_or(i32::MAX);
        sqlx::query!(
            "UPDATE factory_improvement SET failures = $2 WHERE id = $1",
            id,
            held,
        )
        .execute(&mut *tx)
        .await?;
        let rows = sqlx::query!(
            r#"
            SELECT f.run_id, f.kind, f.scorer, f.score, f.reason, f.failed_at,
                coalesce(r.flow, '') AS "flow!"
            FROM factory_improvement_failure f
            JOIN factory_run r ON r.id = f.run_id
            WHERE f.improvement_id = $1
            ORDER BY f.failed_at, f.run_id, f.kind, f.scorer
            "#,
            id,
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ImprovementBegin::Claimed(
            rows.into_iter()
                .map(|row| Failure {
                    run_id: RunId::from_uuid(row.run_id),
                    flow: row.flow,
                    source: source(&row.kind, row.scorer, row.score),
                    reason: row.reason,
                    failed_at: row.failed_at,
                })
                .collect(),
        ))
    }

    #[tracing::instrument(err, skip(self))]
    async fn link_improvement_run(&self, id: Uuid, run_id: RunId) -> Result<bool, Self::Err> {
        let linked = sqlx::query!(
            r#"
            UPDATE factory_improvement SET run_id = $2, state = 'open'
            WHERE id = $1 AND state = 'starting'
            "#,
            id,
            run_id.as_uuid(),
        )
        .execute(&self.pool)
        .await?;
        Ok(linked.rows_affected() == 1)
    }

    #[tracing::instrument(err, skip(self))]
    async fn settle_improvement(&self, id: Uuid, settlement: &Settlement) -> Result<(), Self::Err> {
        let mut tx = self.pool.begin().await?;
        settle(&mut tx, id, settlement).await?;
        tx.commit().await?;
        Ok(())
    }
}

/// Settle the open self-improvement Run `id` in `tx`: its failures are
/// addressed when it merged, dismissed when rejected, else pending again. Settling one settled
/// already changes nothing.
async fn settle(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    settlement: &Settlement,
) -> Result<(), PgRunRepositoryErr> {
    let (state, refusal) = match settlement {
        Settlement::Merged => (ImprovementState::Merged, None),
        Settlement::Closed | Settlement::Rejected => (ImprovementState::Closed, None),
        Settlement::Refused(why) => (
            ImprovementState::Refused,
            Some(cut_bytes(why, MAX_FAILURE_REASON_BYTES)),
        ),
    };
    let settled = sqlx::query!(
        r#"
        UPDATE factory_improvement
        SET state = $2, refusal = $3, settled_at = now()
        WHERE id = $1 AND state IN ('starting', 'open')
        "#,
        id,
        state.as_ref(),
        refusal,
    )
    .execute(&mut **tx)
    .await?;
    if settled.rows_affected() == 0 {
        return Ok(());
    }
    if state == ImprovementState::Merged {
        sqlx::query!(
            r#"
            UPDATE factory_improvement_failure SET addressed_at = now()
            WHERE improvement_id = $1 AND addressed_at IS NULL
            "#,
            id,
        )
        .execute(&mut **tx)
        .await?;
    } else if *settlement == Settlement::Rejected {
        // Held by the rejected improvement, they are never pending again.
        sqlx::query!(
            r#"
            UPDATE factory_improvement_failure SET dismissed_at = now()
            WHERE improvement_id = $1 AND addressed_at IS NULL
            "#,
            id,
        )
        .execute(&mut **tx)
        .await?;
    } else {
        sqlx::query!(
            r#"
            UPDATE factory_improvement_failure SET improvement_id = NULL
            WHERE improvement_id = $1 AND addressed_at IS NULL
            "#,
            id,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Settle as closed, in `tx`, the team `team_id`'s open self-improvement
/// Run on a Definition other than `repo_url` (compared in any case), or on
/// any when `repo_url` is none.
pub(super) async fn close_other_open(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    team_id: Uuid,
    repo_url: Option<&str>,
) -> Result<(), PgRunRepositoryErr> {
    let open = sqlx::query_scalar!(
        r#"
        SELECT id FROM factory_improvement
        WHERE team_id = $1 AND state IN ('starting', 'open')
            AND ($2::text IS NULL OR lower(repo_url) <> lower($2))
        FOR UPDATE
        "#,
        team_id,
        repo_url,
    )
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(id) = open {
        settle(tx, id, &Settlement::Closed).await?;
    }
    Ok(())
}
