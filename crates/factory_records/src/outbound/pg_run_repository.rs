//! PostgreSQL implementation of the [`RunRepository`] port.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::models::{
    EntryId, InvalidName, Outcome, Run, RunEnd, RunId, RunOutcome, Step, StepEnd,
};
use crate::domain::ports::RunRepository;

/// Postgres-backed Run repository.
#[derive(Debug, Clone)]
pub struct PgRunRepository {
    pool: PgPool,
}

impl PgRunRepository {
    /// Create a repository backed by the provided pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Errors produced by the Postgres Run repository.
#[derive(Debug, thiserror::Error)]
pub enum PgRunRepositoryErr {
    /// Underlying database error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// A stored name breaks the rules its column checks.
    #[error(transparent)]
    InvalidName(#[from] InvalidName),
    /// A stored Run outcome is not one this code knows.
    #[error("unknown run outcome {0:?}")]
    UnknownRunOutcome(String),
    /// A stored visit does not fit a visit number.
    #[error("visit {0} is out of range")]
    VisitOutOfRange(i32),
    /// A stored Run or Step has only part of its end.
    #[error("a stored end has an outcome but no time, or the other way round")]
    PartialEnd,
}

struct RunRow {
    id: Uuid,
    request: String,
    created_at: DateTime<Utc>,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
}

impl TryFrom<RunRow> for Run {
    type Error = PgRunRepositoryErr;

    fn try_from(row: RunRow) -> Result<Self, Self::Error> {
        let end = match (row.outcome, row.ended_at) {
            (Some(outcome), Some(ended_at)) => Some(RunEnd {
                outcome: outcome
                    .parse::<RunOutcome>()
                    .map_err(|_| PgRunRepositoryErr::UnknownRunOutcome(outcome))?,
                ended_at,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        Ok(Run {
            id: RunId::from_uuid(row.id),
            request: row.request,
            created_at: row.created_at,
            end,
        })
    }
}

struct StepRow {
    entry: String,
    visit: i32,
    started_at: DateTime<Utc>,
    outcome: Option<String>,
    output: Option<String>,
    ended_at: Option<DateTime<Utc>>,
}

impl TryFrom<StepRow> for Step {
    type Error = PgRunRepositoryErr;

    fn try_from(row: StepRow) -> Result<Self, Self::Error> {
        let end = match (row.outcome, row.output, row.ended_at) {
            (Some(outcome), Some(output), Some(ended_at)) => Some(StepEnd {
                outcome: Outcome::try_from(outcome)?,
                output,
                ended_at,
            }),
            (None, None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        Ok(Step {
            entry: EntryId::try_from(row.entry)?,
            visit: u16::try_from(row.visit)
                .map_err(|_| PgRunRepositoryErr::VisitOutOfRange(row.visit))?,
            started_at: row.started_at,
            end,
        })
    }
}

impl RunRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, request))]
    async fn insert_run(&self, id: RunId, request: &str) -> Result<Run, Self::Err> {
        let row = sqlx::query_as!(
            RunRow,
            r#"
            INSERT INTO factory_run (id, request)
            VALUES ($1, $2)
            RETURNING id, request, created_at, outcome, ended_at
            "#,
            id.as_uuid(),
            request,
        )
        .fetch_one(&self.pool)
        .await?;
        Run::try_from(row)
    }

    #[tracing::instrument(err, skip(self))]
    async fn delete_run(&self, id: RunId) -> Result<(), Self::Err> {
        sqlx::query!("DELETE FROM factory_run WHERE id = $1", id.as_uuid())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn get_run(&self, id: RunId) -> Result<Option<Run>, Self::Err> {
        let row = sqlx::query_as!(
            RunRow,
            r#"
            SELECT id, request, created_at, outcome, ended_at
            FROM factory_run
            WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Run::try_from).transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn list_steps(&self, id: RunId) -> Result<Vec<Step>, Self::Err> {
        let rows = sqlx::query_as!(
            StepRow,
            r#"
            SELECT entry, visit, started_at, outcome, output, ended_at
            FROM factory_step
            WHERE run_id = $1
            ORDER BY started_at, entry, visit
            "#,
            id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Step::try_from).collect()
    }

    #[tracing::instrument(err, skip(self), fields(entry = %entry))]
    async fn start_step(&self, id: RunId, entry: &EntryId, visit: u16) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_step (run_id, entry, visit)
            VALUES ($1, $2, $3)
            ON CONFLICT (run_id, entry, visit) DO NOTHING
            "#,
            id.as_uuid(),
            entry.as_str(),
            i32::from(visit),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, output), fields(entry = %entry, outcome = %outcome))]
    async fn end_step(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
        outcome: &Outcome,
        output: &str,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET outcome = $4, output = $5, ended_at = now()
            WHERE run_id = $1 AND entry = $2 AND visit = $3
            "#,
            id.as_uuid(),
            entry.as_str(),
            i32::from(visit),
            outcome.as_str(),
            output,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn end_run(&self, id: RunId, outcome: RunOutcome) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_run
            SET outcome = $2, ended_at = now()
            WHERE id = $1 AND outcome IS NULL
            "#,
            id.as_uuid(),
            outcome.as_ref(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
