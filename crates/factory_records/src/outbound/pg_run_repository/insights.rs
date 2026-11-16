//! The Postgres Run repository's reads for insights
//! ([`RunInsightsRepository`]).

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::insights::{
    DurationSpread, EntryCount, EntryCountKind, EntrySummary, EntryTrouble, InsightsRecords,
    InsightsScope, RunInsightsRepository, RunSummary, StepSessionRef, TITLE_CHARS,
    TROUBLE_RUNS_PER_ENTRY,
};
use crate::domain::models::{EntryId, RunId, RunOutcome};

struct RunSummaryRow {
    id: Uuid,
    title: String,
    flow: Option<String>,
    created_at: DateTime<Utc>,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
}

impl TryFrom<RunSummaryRow> for RunSummary {
    type Error = PgRunRepositoryErr;

    fn try_from(row: RunSummaryRow) -> Result<Self, Self::Error> {
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
        Ok(RunSummary {
            id: RunId::from_uuid(row.id),
            title: row.title,
            flow: row.flow,
            created_at: row.created_at,
            end,
        })
    }
}

/// A count the schema keeps far below `u32::MAX`: a Run takes at most
/// `MAX_STEPS` Steps, and insights read a capped number of Runs.
fn count(stored: i64) -> u32 {
    u32::try_from(stored).unwrap_or(u32::MAX)
}

/// The spread of `count` durations, from the median and 90th percentile
/// Postgres computed; none when there were none.
fn spread(count_of: i64, median: Option<f64>, p90: Option<f64>) -> Option<DurationSpread> {
    match (median, p90) {
        (Some(median_seconds), Some(p90_seconds)) if count_of > 0 => Some(DurationSpread {
            count: count(count_of),
            median_seconds,
            p90_seconds,
        }),
        _ => None,
    }
}

impl RunInsightsRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, scope), fields(flow = ?scope.flow, cap = scope.cap))]
    async fn read_insights(&self, scope: &InsightsScope) -> Result<InsightsRecords, Self::Err> {
        // Every read sees one snapshot, so a Run that ends meanwhile is not
        // counted running in one read and ended in another. The reads are
        // bounded by the cap, and the timeout bounds them all the same.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *transaction)
            .await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;

        // `factory_run_user_created_idx` reads the person's Runs in the range
        // newest first and stops at the cap. A Flow filter drops the other
        // Flows' Runs from that walk.
        let runs = sqlx::query_as!(
            RunSummaryRow,
            r#"
            SELECT
                id,
                left(split_part(btrim(request), E'\n', 1), $6) AS "title!",
                flow,
                created_at,
                outcome,
                ended_at
            FROM factory_run
            WHERE user_id = $1
              AND created_at >= $2
              AND created_at < $3
              AND ($4::text IS NULL OR flow = $4)
            ORDER BY created_at DESC, id DESC
            LIMIT $5
            "#,
            scope.user_id.as_ref(),
            scope.from,
            scope.to,
            scope.flow.as_deref(),
            i64::from(scope.cap),
            TITLE_CHARS,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(RunSummary::try_from)
        .collect::<Result<Vec<_>, _>>()?;

        if runs.is_empty() {
            transaction.commit().await?;
            return Ok(InsightsRecords::default());
        }

        // The Runs' Steps are read through `factory_step`'s primary key, Run
        // by Run, newest Run first.
        let ids: Vec<Uuid> = runs.iter().map(|run| run.id.as_uuid()).collect();

        // Durations are kept to the millisecond. A Step's duration is its own
        // work: a Gate opens once its Step has ended, so the wait for an
        // answer is measured apart. A Step that a cancelled Run stopped did
        // not finish its work, so its time is left out. A Run visits an entry
        // more than once when the Flow loops back to it, or a reset starts it
        // over; `visit` restarts at a reset, so visits are counted rather
        // than read.
        let entries = sqlx::query!(
            r#"
            WITH steps AS (
                SELECT s.run_id, s.entry, s.outcome, s.started_at, s.ended_at,
                       g.opened_at, g.answered_at
                FROM factory_step s
                LEFT JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
                WHERE s.run_id = ANY($1)
            ),
            by_entry AS (
                SELECT
                    entry,
                    count(*) AS steps,
                    count(*) FILTER (
                        WHERE ended_at IS NOT NULL AND outcome <> 'cancelled'
                    ) AS timed,
                    round(percentile_cont(0.5) WITHIN GROUP (
                        ORDER BY extract(epoch FROM ended_at - started_at)::float8
                    ) FILTER (WHERE ended_at IS NOT NULL AND outcome <> 'cancelled')::numeric, 3)::float8
                        AS duration_median,
                    round(percentile_cont(0.9) WITHIN GROUP (
                        ORDER BY extract(epoch FROM ended_at - started_at)::float8
                    ) FILTER (WHERE ended_at IS NOT NULL AND outcome <> 'cancelled')::numeric, 3)::float8
                        AS duration_p90,
                    count(opened_at) AS gates_opened,
                    count(answered_at) AS gates_answered,
                    round(percentile_cont(0.5) WITHIN GROUP (
                        ORDER BY extract(epoch FROM answered_at - opened_at)::float8
                    )::numeric, 3)::float8 AS wait_median,
                    round(percentile_cont(0.9) WITHIN GROUP (
                        ORDER BY extract(epoch FROM answered_at - opened_at)::float8
                    )::numeric, 3)::float8 AS wait_p90
                FROM steps
                GROUP BY entry
            ),
            visits AS (
                SELECT entry, count(*) AS visits
                FROM steps
                GROUP BY entry, run_id
            ),
            by_run AS (
                SELECT entry,
                       count(*) AS runs,
                       count(*) FILTER (WHERE visits > 1) AS revisited_runs
                FROM visits
                GROUP BY entry
            )
            SELECT
                e.entry AS "entry!",
                e.steps AS "steps!",
                r.runs AS "runs!",
                r.revisited_runs AS "revisited_runs!",
                e.timed AS "timed!",
                e.duration_median,
                e.duration_p90,
                e.gates_opened AS "gates_opened!",
                e.gates_answered AS "gates_answered!",
                e.wait_median,
                e.wait_p90
            FROM by_entry e
            JOIN by_run r USING (entry)
            ORDER BY e.entry
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(EntrySummary {
                entry: EntryId::try_from(row.entry)?,
                steps: count(row.steps),
                runs: count(row.runs),
                revisited_runs: count(row.revisited_runs),
                duration: spread(row.timed, row.duration_median, row.duration_p90),
                gates_opened: count(row.gates_opened),
                gates_answered: count(row.gates_answered),
                gate_wait: spread(row.gates_answered, row.wait_median, row.wait_p90),
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        // The Runs' Steps, with their Gates, are read once by primary key;
        // both counts come from that one read.
        let counts = sqlx::query!(
            r#"
            WITH steps AS MATERIALIZED (
                SELECT s.entry, s.outcome, g.answer
                FROM factory_step s
                LEFT JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
                WHERE s.run_id = ANY($1)
            )
            SELECT entry AS "entry!", 'outcome' AS "kind!", outcome AS "value!", count(*) AS "count!"
            FROM steps
            WHERE outcome IS NOT NULL
            GROUP BY entry, outcome
            UNION ALL
            SELECT entry, 'answer', answer, count(*)
            FROM steps
            WHERE answer IS NOT NULL
            GROUP BY entry, answer
            ORDER BY 1, 2, 3
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            let kind = if row.kind == "answer" {
                EntryCountKind::Answer
            } else {
                EntryCountKind::Outcome
            };
            Ok(EntryCount {
                entry: EntryId::try_from(row.entry)?,
                kind,
                value: row.value,
                count: count(row.count),
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        // For each entry, the newest Runs in which it failed or was visited
        // more than once. The ids come newest first, so their place orders
        // the Runs.
        let trouble = sqlx::query!(
            r#"
            SELECT entry AS "entry!", run_id AS "run_id!", failed AS "failed!", visits AS "visits!"
            FROM (
                SELECT
                    t.entry,
                    t.run_id,
                    t.failed,
                    t.visits,
                    row_number() OVER (PARTITION BY t.entry ORDER BY r.place) AS rank
                FROM (
                    SELECT
                        entry,
                        run_id,
                        coalesce(bool_or(outcome = 'failed'), false) AS failed,
                        count(*) AS visits
                    FROM factory_step
                    WHERE run_id = ANY($1)
                    GROUP BY entry, run_id
                ) t
                JOIN unnest($1::uuid[]) WITH ORDINALITY AS r(id, place) ON r.id = t.run_id
                WHERE t.failed OR t.visits > 1
            ) ranked
            WHERE rank <= $2
            ORDER BY entry, rank
            "#,
            &ids,
            TROUBLE_RUNS_PER_ENTRY,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(EntryTrouble {
                entry: EntryId::try_from(row.entry)?,
                run_id: RunId::from_uuid(row.run_id),
                failed: row.failed,
                visits: count(row.visits),
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        // Fenced, so the Runs' ids find their Steps by primary key, rather
        // than every agent Step being walked through the session index.
        let sessions = sqlx::query!(
            r#"
            SELECT run_id, entry, session_id AS "session_id!"
            FROM (
                SELECT run_id, entry, session_id
                FROM factory_step
                WHERE run_id = ANY($1)
                OFFSET 0
            ) steps
            WHERE session_id IS NOT NULL
            "#,
            &ids,
        )
        .fetch_all(&mut *transaction)
        .await?
        .into_iter()
        .map(|row| {
            Ok(StepSessionRef {
                run_id: RunId::from_uuid(row.run_id),
                entry: EntryId::try_from(row.entry)?,
                session_id: row.session_id,
            })
        })
        .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;

        transaction.commit().await?;
        Ok(InsightsRecords {
            runs,
            entries,
            counts,
            trouble,
            sessions,
        })
    }
}
