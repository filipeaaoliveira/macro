//! The Postgres Run repository's child tasks of `split` Steps
//! (`factory_split_task`).

#[cfg(test)]
mod test;

use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::models::{RunId, StepKey};
use crate::domain::split_tasks::SplitTask;

impl PgRunRepository {
    /// Keep the child at `place` the Step `key` filed, unless it keeps one
    /// there already.
    pub(super) async fn insert_split_task(
        &self,
        key: StepKey,
        place: u16,
        task_id: Uuid,
        name: &str,
    ) -> Result<(), PgRunRepositoryErr> {
        sqlx::query!(
            r#"
            INSERT INTO factory_split_task (run_id, seq, place, task_id, name)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (run_id, seq, place) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(place),
            task_id,
            name,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The children the Run `id`'s Steps filed, through the primary key.
    pub(super) async fn select_split_tasks(
        &self,
        id: RunId,
    ) -> Result<Vec<SplitTask>, PgRunRepositoryErr> {
        sqlx::query!(
            r#"
            SELECT seq, place, task_id, name
            FROM factory_split_task
            WHERE run_id = $1
            ORDER BY seq, place
            "#,
            id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            Ok(SplitTask {
                seq: u16::try_from(row.seq).map_err(|_| PgRunRepositoryErr::OutOfRange(row.seq))?,
                place: u16::try_from(row.place)
                    .map_err(|_| PgRunRepositoryErr::OutOfRange(row.place))?,
                task_id: row.task_id,
                name: row.name,
            })
        })
        .collect()
    }
}
