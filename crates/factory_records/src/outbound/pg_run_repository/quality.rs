//! The Postgres Run repository's CI waits (`factory_ci_wait`,
//! `factory_ci_job`), and its reads for CI health, review quality, cost and
//! autonomy ([`RunQualityRepository`]).

#[cfg(test)]
mod test;

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::ci_retries::{quarantines_of, retry_result};
use super::review_rounds::stored_word;
use super::{PgRunRepository, PgRunRepositoryErr, number};
use crate::domain::evaluation::BENCHMARK_FLOW;
use crate::domain::insights::{RunSummary, TITLE_CHARS};
use crate::domain::models::{RunId, RunOutcome};
use crate::domain::quality::{
    BlockerRecord, CiJob, CiJobResult, CiRetryRecord, CiVerdict, CiWait, CiWaitRecord,
    FLAKY_PERIOD, LastCiWait, QualityRecords, QualityRun, QualityScope, QualitySession,
    ReviewRoundRecord, ReviewerRecord, RunQualityRepository,
};
use crate::domain::review_rounds::{DisputeRuling, FixStatus, ReviewRoundOutcome};

/// Keep `wait`, replacing the one its Step kept unless that read the same
/// head and verdict: the last attempt's wait is the one the Step routed.
pub(super) async fn record_ci_wait(pool: &PgPool, wait: &CiWait) -> Result<(), PgRunRepositoryErr> {
    let names: Vec<String> = wait.jobs.iter().map(|job| job.name.clone()).collect();
    let results: Vec<String> = wait
        .jobs
        .iter()
        .map(|job| job.result.as_str().to_string())
        .collect();
    let steps: Vec<Option<String>> = wait
        .jobs
        .iter()
        .map(|job| job.failed_step.clone())
        .collect();
    let failures: Vec<Option<String>> = wait.jobs.iter().map(|job| job.failure.clone()).collect();
    let replaced: Vec<bool> = wait.jobs.iter().map(|job| job.replaced).collect();
    let mut transaction = pool.begin().await?;
    // A wait of the same head and verdict is kept as it was, with its jobs
    // and the time it ended; another replaces it.
    let changed = sqlx::query_scalar!(
        r#"
        INSERT INTO factory_ci_wait (run_id, seq, head_sha, verdict)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (run_id, seq) DO UPDATE
        SET head_sha = EXCLUDED.head_sha, verdict = EXCLUDED.verdict, recorded_at = now()
        WHERE (factory_ci_wait.head_sha, factory_ci_wait.verdict)
            IS DISTINCT FROM (EXCLUDED.head_sha, EXCLUDED.verdict)
        RETURNING seq
        "#,
        wait.key.run_id.as_uuid(),
        i32::from(wait.key.seq),
        wait.head_sha,
        wait.verdict.as_str(),
    )
    .fetch_optional(&mut *transaction)
    .await?;
    if changed.is_none() {
        transaction.commit().await?;
        return Ok(());
    }
    sqlx::query!(
        "DELETE FROM factory_ci_job WHERE run_id = $1 AND seq = $2",
        wait.key.run_id.as_uuid(),
        i32::from(wait.key.seq),
    )
    .execute(&mut *transaction)
    .await?;
    // A name seen twice on one head, as a check run and a commit status of
    // the same name, keeps its first.
    sqlx::query!(
        r#"
        INSERT INTO factory_ci_job (run_id, seq, name, result, failed_step, failure, replaced)
        SELECT $1, $2, job.name, job.result, job.failed_step, job.failure, job.replaced
        FROM unnest($3::text[], $4::text[], $5::text[], $6::text[], $7::bool[])
            WITH ORDINALITY AS job(name, result, failed_step, failure, replaced, place)
        ORDER BY job.place
        ON CONFLICT (run_id, seq, replaced, name) DO NOTHING
        "#,
        wait.key.run_id.as_uuid(),
        i32::from(wait.key.seq),
        &names,
        &results,
        &steps as &[Option<String>],
        &failures as &[Option<String>],
        &replaced,
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

/// The head of `run_id`'s last kept wait before the Step at `before`, with
/// the jobs it kept there (not those of heads it replaced).
pub(super) async fn last_ci_wait(
    pool: &PgPool,
    run_id: RunId,
    before: u16,
) -> Result<Option<LastCiWait>, PgRunRepositoryErr> {
    let row = sqlx::query!(
        r#"
        SELECT
            w.head_sha,
            coalesce(
                array_agg(j.name ORDER BY j.name) FILTER (WHERE j.name IS NOT NULL),
                '{}'
            ) AS "jobs!"
        FROM (
            SELECT run_id, seq, head_sha
            FROM factory_ci_wait
            WHERE run_id = $1 AND seq < $2
            ORDER BY seq DESC
            LIMIT 1
        ) w
        LEFT JOIN factory_ci_job j
            ON j.run_id = w.run_id AND j.seq = w.seq AND NOT j.replaced
        GROUP BY w.head_sha
        "#,
        run_id.as_uuid(),
        i32::from(before),
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| LastCiWait {
        head_sha: row.head_sha,
        jobs: row.jobs,
    }))
}

struct QualityRunRow {
    id: Uuid,
    title: String,
    flow: Option<String>,
    created_at: DateTime<Utc>,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    repo_url: Option<String>,
    pull_request_url: Option<String>,
    merged: bool,
    person_answered: bool,
    steered: bool,
}

impl TryFrom<QualityRunRow> for QualityRun {
    type Error = PgRunRepositoryErr;

    fn try_from(row: QualityRunRow) -> Result<Self, Self::Error> {
        let end = match (row.outcome, row.ended_at) {
            (Some(outcome), Some(ended_at)) => Some((
                outcome
                    .parse::<RunOutcome>()
                    .map_err(|_| PgRunRepositoryErr::UnknownRunOutcome(outcome))?,
                ended_at,
            )),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        Ok(QualityRun {
            run: RunSummary {
                id: RunId::from_uuid(row.id),
                title: row.title,
                flow: row.flow,
                created_at: row.created_at,
                end,
            },
            repository: row.repo_url,
            pull_request: row.pull_request_url,
            merged: row.merged,
            person_answered: row.person_answered,
            steered: row.steered,
            sessions: Vec::new(),
        })
    }
}

fn verdict(word: String) -> Result<CiVerdict, PgRunRepositoryErr> {
    CiVerdict::parse(&word).ok_or(PgRunRepositoryErr::UnknownCiWord(word))
}

fn job_result(word: String) -> Result<CiJobResult, PgRunRepositoryErr> {
    CiJobResult::parse(&word).ok_or(PgRunRepositoryErr::UnknownCiWord(word))
}

fn round_outcome(word: Option<String>) -> Result<Option<ReviewRoundOutcome>, PgRunRepositoryErr> {
    word.map(|word| match word.as_str() {
        "pass" => Ok(ReviewRoundOutcome::Pass),
        "changes" => Ok(ReviewRoundOutcome::Changes),
        "failed" => Ok(ReviewRoundOutcome::Failed),
        _ => Err(PgRunRepositoryErr::UnknownReviewWord(word)),
    })
    .transpose()
}

/// The most repositories [`QualityRecords::repositories`] lists.
const REPOSITORIES_LISTED: i64 = 100;

impl RunQualityRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, scope), fields(flow = ?scope.runs.flow, repository = ?scope.repository))]
    async fn read_quality(&self, scope: &QualityScope) -> Result<QualityRecords, Self::Err> {
        // One snapshot for every read, bounded by the cap and a timeout, as
        // the Run insights read.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let runs_scope = &scope.runs;

        // The person's Runs in the range, newest first, through
        // `factory_run_user_created_idx`, one past the cap. Each says
        // whether it merged, and whether a person stepped in: answered one
        // of its Gates in person (not a Standing Rule, the stand-in or an
        // agent), or steered one of its Steps. Benchmark trials are no
        // one's work and are never read.
        let mut runs = sqlx::query_as!(
            QualityRunRow,
            r#"
            SELECT
                r.id,
                left(split_part(btrim(r.request), E'\n', 1), $6) AS "title!",
                r.flow,
                r.created_at,
                r.outcome,
                r.ended_at,
                r.repo_url,
                r.pull_request_url,
                EXISTS (SELECT 1 FROM factory_merge m WHERE m.run_id = r.id) AS "merged!",
                EXISTS (
                    SELECT 1 FROM factory_gate g
                    WHERE g.run_id = r.id AND g.seq >= COALESCE(r.first_seq, 1)
                        AND g.answer IS NOT NULL AND g.rule IS NULL
                        AND NOT g.by_stand_in AND g.agent_session_id IS NULL
                ) AS "person_answered!",
                EXISTS (SELECT 1 FROM factory_step_steer t WHERE t.run_id = r.id) AS "steered!"
            FROM factory_run r
            WHERE r.user_id = $1
              AND r.created_at >= $2
              AND r.created_at < $3
              AND ($4::text IS NULL OR r.flow = $4)
              AND r.flow IS DISTINCT FROM $7
              AND ($8::text IS NULL OR lower(r.repo_url) = lower($8))
            ORDER BY r.created_at DESC, r.id DESC
            LIMIT $5
            "#,
            runs_scope.user_id.as_ref(),
            runs_scope.from,
            runs_scope.to,
            runs_scope.flow.as_deref(),
            i64::from(runs_scope.cap) + 1,
            TITLE_CHARS,
            BENCHMARK_FLOW,
            scope.repository.as_deref(),
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(QualityRun::try_from)
        .collect::<Result<Vec<_>, _>>()?;
        let capped = runs.len() > usize::from(runs_scope.cap);
        runs.truncate(usize::from(runs_scope.cap));

        // The repositories of the same Runs, whatever the repository the
        // scope names, to pick one from: each once whatever its case, as
        // GitHub names repositories case-insensitively.
        let repositories = sqlx::query_scalar!(
            r#"
            SELECT min(repo_url) AS "repo_url!"
            FROM (
                SELECT repo_url
                FROM factory_run
                WHERE user_id = $1
                  AND created_at >= $2
                  AND created_at < $3
                  AND ($4::text IS NULL OR flow = $4)
                  AND flow IS DISTINCT FROM $6
                ORDER BY created_at DESC, id DESC
                LIMIT $5
            ) runs
            WHERE repo_url IS NOT NULL
            GROUP BY lower(repo_url)
            ORDER BY lower(repo_url)
            LIMIT $7
            "#,
            runs_scope.user_id.as_ref(),
            runs_scope.from,
            runs_scope.to,
            runs_scope.flow.as_deref(),
            i64::from(runs_scope.cap),
            BENCHMARK_FLOW,
            REPOSITORIES_LISTED,
        )
        .fetch_all(&mut *transaction)
        .await?;

        if runs.is_empty() {
            transaction.commit().await?;
            return Ok(QualityRecords {
                capped,
                repositories,
                ..QualityRecords::default()
            });
        }
        let ids: Vec<Uuid> = runs.iter().map(|run| run.run.id.as_uuid()).collect();

        // A retry's Steps before its own (`first_seq`) are the retried
        // Run's, kept for its later Steps to read: they, their Gates,
        // sessions and review rounds count once, on the Run that took them,
        // so the reads below leave them out.
        //
        // The Runs' agent sessions and the bot that ran each, through
        // `factory_step`'s primary key; a Step that went on after a question
        // shares the asking Step's session, which counts once. The sessions
        // Steps left count too, as the Run's token budget counts them, with
        // no bot: a Step that moves on records its new Agent's bot, and the
        // one it left keeps none, so its spend reads as unknown rather than
        // as the new bot's. A session keeps one bot, which a Step from
        // before Steps recorded it lacks.
        let sessions = sqlx::query!(
            r#"
            SELECT run_id AS "run_id!", session_id AS "session_id!",
                (array_agg(bot_id) FILTER (WHERE bot_id IS NOT NULL))[1] AS bot_id
            FROM (
                (
                    SELECT run_id, seq, session_id, bot_id
                    FROM factory_step
                    WHERE run_id = ANY($1)
                    OFFSET 0
                )
                UNION ALL
                SELECT u.run_id, u.seq, u.session_id,
                    CASE WHEN u.session_id = s.session_id THEN s.bot_id END
                FROM factory_step_used_session u
                JOIN factory_step s ON s.run_id = u.run_id AND s.seq = u.seq
                WHERE u.run_id = ANY($1)
            ) steps
            WHERE session_id IS NOT NULL AND seq >= (SELECT COALESCE(k.first_seq, 1) FROM factory_run k WHERE k.id = steps.run_id)
            GROUP BY run_id, session_id
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?;
        let mut by_run: HashMap<Uuid, Vec<QualitySession>> = HashMap::new();
        for row in sessions {
            by_run.entry(row.run_id).or_default().push(QualitySession {
                id: row.session_id,
                bot: row.bot_id,
            });
        }
        for run in &mut runs {
            if let Some(mut sessions) = by_run.remove(&run.run.id.as_uuid()) {
                sessions.sort_by_key(|session| session.id);
                run.sessions = sessions;
            }
        }

        // Each `ci` wait, with when its Step started, and the jobs it saw.
        let waits = sqlx::query!(
            r#"
            SELECT w.run_id, w.seq, s.started_at, w.recorded_at, w.verdict
            FROM factory_ci_wait w
            JOIN factory_step s ON s.run_id = w.run_id AND s.seq = w.seq
            WHERE w.run_id = ANY($1)
            ORDER BY w.run_id, w.seq
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?;
        let jobs = sqlx::query!(
            r#"
            SELECT run_id, seq, name, result, failed_step, failure, replaced
            FROM factory_ci_job
            WHERE run_id = ANY($1)
            ORDER BY run_id, seq, replaced, name
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?;
        let mut jobs_by_wait: HashMap<(Uuid, i32), Vec<CiJob>> = HashMap::new();
        for row in jobs {
            jobs_by_wait
                .entry((row.run_id, row.seq))
                .or_default()
                .push(CiJob {
                    name: row.name,
                    result: job_result(row.result)?,
                    failed_step: row.failed_step,
                    failure: row.failure,
                    replaced: row.replaced,
                });
        }
        let ci_waits = waits
            .into_iter()
            .map(|row| {
                Ok(CiWaitRecord {
                    run_id: RunId::from_uuid(row.run_id),
                    seq: number(row.seq)?,
                    started_at: row.started_at,
                    ended_at: row.recorded_at,
                    verdict: verdict(row.verdict)?,
                    jobs: jobs_by_wait
                        .remove(&(row.run_id, row.seq))
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        // The review rounds, their reviewers' Steps with the Role and the
        // Agent of each, and the Blockers they found.
        let review_rounds = sqlx::query!(
            r#"
            SELECT run_id, seq, started_at, outcome
            FROM factory_review_round rr
            WHERE run_id = ANY($1) AND seq >= (SELECT COALESCE(k.first_seq, 1) FROM factory_run k WHERE k.id = rr.run_id)
            ORDER BY run_id, seq
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(ReviewRoundRecord {
                run_id: RunId::from_uuid(row.run_id),
                seq: number(row.seq)?,
                started_at: row.started_at,
                outcome: round_outcome(row.outcome)?,
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;
        let reviewers = sqlx::query!(
            r#"
            SELECT a.run_id, a.seq, a.round_seq, s.role, s.agent
            FROM factory_reviewer_answer a
            JOIN factory_step s ON s.run_id = a.run_id AND s.seq = a.seq
            WHERE a.run_id = ANY($1) AND a.seq >= (SELECT COALESCE(k.first_seq, 1) FROM factory_run k WHERE k.id = a.run_id)
            ORDER BY a.run_id, a.seq
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(ReviewerRecord {
                run_id: RunId::from_uuid(row.run_id),
                seq: number(row.seq)?,
                round_seq: number(row.round_seq)?,
                role: row.role,
                agent: row.agent,
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;
        let blockers = sqlx::query!(
            r#"
            SELECT
                run_id, seq, round_seq, problem, fix_status, ruling,
                kind = 'dismissed' AS "dismissed!"
            FROM factory_review_finding f
            WHERE run_id = ANY($1) AND kind IN ('blocker', 'dismissed')
                AND seq >= (SELECT COALESCE(k.first_seq, 1) FROM factory_run k WHERE k.id = f.run_id)
            ORDER BY run_id, seq, place
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(BlockerRecord {
                run_id: RunId::from_uuid(row.run_id),
                seq: number(row.seq)?,
                round_seq: number(row.round_seq)?,
                problem: row.problem,
                fix_status: stored_word(row.fix_status, FixStatus::from_word)?,
                ruling: stored_word(row.ruling, DisputeRuling::from_word)?,
                dismissed: row.dismissed,
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        // The CI jobs the Runs' `ci` Steps reran, and the jobs quarantined
        // in the repositories read.
        let ci_retries = sqlx::query!(
            r#"
            SELECT run_id, name, result
            FROM factory_ci_retry
            WHERE run_id = ANY($1)
            ORDER BY run_id, seq, name
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(CiRetryRecord {
                run_id: RunId::from_uuid(row.run_id),
                name: row.name,
                result: retry_result(row.result)?,
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;
        let quarantined_in: Vec<String> = match &scope.repository {
            Some(repository) => vec![repository.to_lowercase()],
            None => repositories
                .iter()
                .map(|repository| repository.to_lowercase())
                .collect(),
        };
        let ci_quarantines =
            quarantines_of(&mut transaction, &quarantined_in, Utc::now() - FLAKY_PERIOD).await?;

        transaction.commit().await?;
        Ok(QualityRecords {
            runs,
            capped,
            repositories,
            ci_waits,
            review_rounds,
            reviewers,
            blockers,
            ci_retries,
            ci_quarantines,
        })
    }
}
