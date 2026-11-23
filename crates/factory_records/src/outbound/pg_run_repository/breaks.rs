//! The Postgres Run repository's break records (`factory_break_record`).

#[cfg(test)]
mod test;

use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::breaks::{BreakRecord, BreakStatus, NewBreakRecord, RunsBreakRecords};
use crate::domain::models::{RunId, StepKey};

impl PgRunRepository {
    /// Keep `record` for the Step `key`, unless that Step keeps one for its
    /// test already: the first stays.
    pub(super) async fn insert_break(
        &self,
        key: StepKey,
        record: &NewBreakRecord,
    ) -> Result<(), PgRunRepositoryErr> {
        sqlx::query!(
            r#"
            INSERT INTO factory_break_record
                (run_id, seq, test, file, old_text, new_text, command, status, checked_commit,
                 output)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (run_id, seq, test) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            record.test,
            record.file,
            record.old_text,
            record.new_text,
            record.command,
            record.status.as_ref(),
            record.checked_commit,
            record.output,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The break records of the Runs `ids`, by Run.
    pub(super) async fn select_runs_breaks(
        &self,
        ids: &[RunId],
    ) -> Result<RunsBreakRecords, PgRunRepositoryErr> {
        let ids: Vec<Uuid> = ids.iter().map(RunId::as_uuid).collect();
        let rows = sqlx::query!(
            r#"
            SELECT run_id, seq, test, file, old_text, new_text, command, status, checked_commit,
                   output, recorded_at
            FROM factory_break_record
            WHERE run_id = ANY($1)
            ORDER BY run_id, seq, recorded_at, test
            "#,
            &ids,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut records = RunsBreakRecords::new();
        for row in rows {
            let status = row
                .status
                .parse::<BreakStatus>()
                .map_err(|_| PgRunRepositoryErr::UnknownBreakStatus(row.status.clone()))?;
            records
                .entry(RunId::from_uuid(row.run_id))
                .or_default()
                .push(BreakRecord {
                    seq: u16::try_from(row.seq)
                        .map_err(|_| PgRunRepositoryErr::OutOfRange(row.seq))?,
                    test: row.test,
                    file: row.file,
                    old_text: row.old_text,
                    new_text: row.new_text,
                    command: row.command,
                    status,
                    checked_commit: row.checked_commit,
                    output: row.output,
                    recorded_at: row.recorded_at,
                });
        }
        Ok(records)
    }
}
