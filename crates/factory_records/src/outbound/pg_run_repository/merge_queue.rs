//! The Postgres Run repository's merge queues (`factory_merge_queue`) and
//! the updates of Runs' branches with their base (`factory_branch_update`).

#[cfg(test)]
mod test;

use chrono::TimeDelta;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::merge_queue::{BranchUpdate, MergeQueueRepository};
use crate::domain::models::{RunId, StepKey};

/// How long a merge turn is held at most: a transaction idle longer, as
/// one a worker that stopped answering left open, is ended by the
/// database, which lets the turn go. A merge's reads and GitHub's merge take
/// seconds.
const TURN_HELD_AT_MOST: &str = "300000";

/// A Run's merge turn: a transaction that holds its base's advisory lock,
/// rolled back, and so let go, when it is dropped.
pub struct PgMergeTurn {
    _transaction: sqlx::Transaction<'static, sqlx::Postgres>,
}

impl std::fmt::Debug for PgMergeTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PgMergeTurn")
    }
}

impl MergeQueueRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;
    type Turn = PgMergeTurn;

    /// The turn is a transaction-scoped advisory lock keyed by the base as
    /// the queue keys it, tried rather than waited for, so no connection
    /// waits on it. The base is read first, outside the turn's transaction,
    /// which then only takes the lock: it holds no row or table lock of any
    /// factory table, so nothing it may hold for minutes stands in the way
    /// of a write to the Run or a migration of its table.
    #[tracing::instrument(err, skip(self))]
    async fn take_merge_turn(&self, run_id: RunId) -> Result<Option<PgMergeTurn>, Self::Err> {
        let key = sqlx::query_scalar!(
            r#"
            SELECT hashtextextended(
                'factory_merge_turn:'
                    || lower(regexp_replace(btrim(repo_url), '\.git$', ''))
                    || ' ' || base_branch,
                0
            ) AS "key!"
            FROM factory_run
            WHERE id = $1 AND repo_url IS NOT NULL AND base_branch IS NOT NULL
            "#,
            run_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query_scalar!(
            r#"SELECT set_config('idle_in_transaction_session_timeout', $1, true) AS "set!""#,
            TURN_HELD_AT_MOST,
        )
        .fetch_one(&mut *transaction)
        .await?;
        // A Run on no repository has no base another Run merges into: its
        // turn is a transaction that holds nothing.
        let Some(key) = key else {
            return Ok(Some(PgMergeTurn {
                _transaction: transaction,
            }));
        };
        let taken = sqlx::query_scalar!(r#"SELECT pg_try_advisory_xact_lock($1) AS "taken!""#, key)
            .fetch_one(&mut *transaction)
            .await?;
        if !taken {
            return Ok(None);
        }
        Ok(Some(PgMergeTurn {
            _transaction: transaction,
        }))
    }

    /// The queue is keyed by the Run's repository URL lower-cased, trimmed
    /// and without a `.git` ending, so one repository is one queue whatever
    /// the case a Run named it in.
    #[tracing::instrument(err, skip(self))]
    async fn enter_merge_queue(&self, run_id: RunId) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_merge_queue (run_id, repo_url, base_branch)
            SELECT id, lower(regexp_replace(btrim(repo_url), '\.git$', '')), base_branch
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
                AND other.place < mine.place
            JOIN factory_run run ON run.id = other.run_id
            WHERE mine.run_id = $1
                AND run.ended_at IS NULL
                AND (other.left_at IS NULL
                    OR other.left_at > now() - make_interval(secs => $2))
            ORDER BY other.place
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
    async fn reviewed_head(&self, run_id: RunId) -> Result<Option<String>, Self::Err> {
        let head = sqlx::query_scalar!(
            "SELECT head_sha FROM factory_reviewed_head WHERE run_id = $1",
            run_id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(head)
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_reviewed_head(&self, run_id: RunId, head: &str) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_reviewed_head (run_id, head_sha)
            VALUES ($1, $2)
            ON CONFLICT (run_id) DO UPDATE
            SET head_sha = EXCLUDED.head_sha, reviewed_at = now()
            "#,
            run_id.as_uuid(),
            head,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
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
