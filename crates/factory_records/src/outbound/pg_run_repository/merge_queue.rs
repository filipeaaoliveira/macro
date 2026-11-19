//! The Postgres Run repository's merge queues (`factory_merge_queue`) and
//! the updates of Runs' branches with their base (`factory_branch_update`).

#[cfg(test)]
mod test;

use chrono::TimeDelta;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::merge_queue::{BranchUpdate, MergeQueueRepository};
use crate::domain::models::{RunId, StepKey};

impl MergeQueueRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn enter_merge_queue(&self, run_id: RunId) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_merge_queue (run_id, repo_url, base_branch)
            SELECT id, repo_url, base_branch
            FROM factory_run
            WHERE id = $1 AND repo_url IS NOT NULL AND base_branch IS NOT NULL
            ON CONFLICT (run_id) DO UPDATE SET left_at = NULL
            "#,
            run_id.as_uuid(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn merge_queue_ahead(
        &self,
        run_id: RunId,
        place_kept: TimeDelta,
    ) -> Result<Vec<RunId>, Self::Err> {
        // The queue of one base is a handful of Runs, read through its
        // index from the Run's own row.
        let kept_secs = place_kept.as_seconds_f64();
        let ahead = sqlx::query_scalar!(
            r#"
            SELECT other.run_id
            FROM factory_merge_queue mine
            JOIN factory_merge_queue other
                ON other.repo_url = mine.repo_url
                AND other.base_branch = mine.base_branch
                AND (other.entered_at, other.run_id) < (mine.entered_at, mine.run_id)
            JOIN factory_run run ON run.id = other.run_id
            WHERE mine.run_id = $1
                AND run.ended_at IS NULL
                AND (other.left_at IS NULL
                    OR other.left_at > now() - make_interval(secs => $2))
            ORDER BY other.entered_at, other.run_id
            "#,
            run_id.as_uuid(),
            kept_secs,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(ahead.into_iter().map(RunId::from_uuid).collect())
    }

    #[tracing::instrument(err, skip(self))]
    async fn leave_merge_queue(&self, run_id: RunId) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_merge_queue
            SET left_at = now()
            WHERE run_id = $1 AND left_at IS NULL
            "#,
            run_id.as_uuid(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn drop_from_merge_queue(&self, run_id: RunId) -> Result<(), Self::Err> {
        sqlx::query!(
            "DELETE FROM factory_merge_queue WHERE run_id = $1",
            run_id.as_uuid(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn branch_update(&self, key: StepKey) -> Result<Option<BranchUpdate>, Self::Err> {
        let row = sqlx::query_as!(
            BranchUpdate,
            r#"
            SELECT base_sha, from_head, conflict, updated_head
            FROM factory_branch_update
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row)
    }

    #[tracing::instrument(err, skip(self))]
    async fn keep_branch_update(
        &self,
        key: StepKey,
        update: &BranchUpdate,
    ) -> Result<BranchUpdate, Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_branch_update
                (run_id, seq, base_sha, from_head, conflict, updated_head)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (run_id, seq) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            update.base_sha,
            update.from_head,
            update.conflict,
            update.updated_head,
        )
        .execute(&self.pool)
        .await?;
        self.branch_update(key)
            .await?
            .ok_or(PgRunRepositoryErr::MissingBranchUpdate)
    }

    #[tracing::instrument(err, skip(self))]
    async fn restart_branch_update(
        &self,
        key: StepKey,
        update: &BranchUpdate,
    ) -> Result<BranchUpdate, Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_branch_update
            SET base_sha = $3, from_head = $4, conflict = $5, updated_head = $6
            WHERE run_id = $1 AND seq = $2 AND NOT conflict AND updated_head IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            update.base_sha,
            update.from_head,
            update.conflict,
            update.updated_head,
        )
        .execute(&self.pool)
        .await?;
        self.branch_update(key)
            .await?
            .ok_or(PgRunRepositoryErr::MissingBranchUpdate)
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_updated_head(&self, key: StepKey, head: &str) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_branch_update
            SET updated_head = $3
            WHERE run_id = $1 AND seq = $2 AND NOT conflict AND updated_head IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            head,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
