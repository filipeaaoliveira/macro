//! The Postgres Run repository's merges (`factory_merge`) and the holds on
//! the base branches Runs merge to (`factory_main_hold`).

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::main_hold::{FactoryMerge, MainHold, MainHoldRepository};
use crate::domain::models::{Repository, RunId};

struct HoldRow {
    held_since: Option<DateTime<Utc>>,
    head_sha: Option<String>,
    failed: Option<String>,
    task_claimed_at: Option<DateTime<Utc>>,
    task_id: Option<String>,
}

/// The hold a row keeps, if the base was last seen red.
fn hold(row: HoldRow) -> Result<Option<MainHold>, PgRunRepositoryErr> {
    let (since, head_sha, failed) = match (row.held_since, row.head_sha, row.failed) {
        (Some(since), Some(head_sha), Some(failed)) => (since, head_sha, failed),
        (None, None, None) => return Ok(None),
        _ => return Err(PgRunRepositoryErr::PartialHold),
    };
    let task_id = row
        .task_id
        .map(|id| Uuid::parse_str(&id).map_err(|_| PgRunRepositoryErr::InvalidTaskId(id)))
        .transpose()?;
    Ok(Some(MainHold {
        since,
        head_sha,
        failed,
        task_id,
        task_claimed: row.task_claimed_at.is_some(),
    }))
}

impl MainHoldRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn record_merge(
        &self,
        run_id: RunId,
        number: u64,
        url: &str,
        merged_at: DateTime<Utc>,
    ) -> Result<(), Self::Err> {
        let number = i64::try_from(number).map_err(|_| PgRunRepositoryErr::NumberOutOfRange)?;
        sqlx::query!(
            r#"
            INSERT INTO factory_merge
                (run_id, repo_url, base_branch, pull_request_number, pull_request_url, merged_at)
            SELECT id, repo_url, base_branch, $2, $3, $4
            FROM factory_run
            WHERE id = $1 AND repo_url IS NOT NULL AND base_branch IS NOT NULL
            ON CONFLICT (run_id) DO NOTHING
            "#,
            run_id.as_uuid(),
            number,
            url,
            merged_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn main_hold(&self, base: &Repository) -> Result<Option<MainHold>, Self::Err> {
        let row = sqlx::query_as!(
            HoldRow,
            r#"
            SELECT held_since, head_sha, failed, task_claimed_at, task_id
            FROM factory_main_hold
            WHERE repo_url = $1 AND base_branch = $2
            "#,
            base.url,
            base.base_branch,
        )
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) => hold(row),
            None => Ok(None),
        }
    }

    #[tracing::instrument(err, skip(self, failed))]
    async fn hold_main(
        &self,
        base: &Repository,
        head_sha: &str,
        failed: &str,
    ) -> Result<MainHold, Self::Err> {
        let row = sqlx::query_as!(
            HoldRow,
            r#"
            INSERT INTO factory_main_hold (repo_url, base_branch, held_since, head_sha, failed)
            VALUES ($1, $2, now(), $3, $4)
            ON CONFLICT (repo_url, base_branch) DO UPDATE
            SET held_since = COALESCE(factory_main_hold.held_since, EXCLUDED.held_since),
                head_sha = EXCLUDED.head_sha,
                failed = EXCLUDED.failed
            RETURNING held_since, head_sha, failed, task_claimed_at, task_id
            "#,
            base.url,
            base.base_branch,
            head_sha,
            failed,
        )
        .fetch_one(&self.pool)
        .await?;
        hold(row)?.ok_or(PgRunRepositoryErr::PartialHold)
    }

    #[tracing::instrument(err, skip(self))]
    async fn clear_main_hold(
        &self,
        base: &Repository,
        seen_at: DateTime<Utc>,
    ) -> Result<(), Self::Err> {
        // A read that started before the last green one leaves its time.
        sqlx::query!(
            r#"
            INSERT INTO factory_main_hold (repo_url, base_branch, last_green_at)
            VALUES ($1, $2, $3)
            ON CONFLICT (repo_url, base_branch) DO UPDATE
            SET last_green_at = GREATEST(factory_main_hold.last_green_at, EXCLUDED.last_green_at),
                held_since = NULL,
                head_sha = NULL,
                failed = NULL,
                task_claimed_at = NULL,
                task_id = NULL
            "#,
            base.url,
            base.base_branch,
            seen_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn suspect_merges(
        &self,
        base: &Repository,
        limit: u16,
    ) -> Result<Vec<FactoryMerge>, Self::Err> {
        // `factory_merge_base_idx` walks the base's merges newest first and
        // stops at the last green time, or at the limit.
        let rows = sqlx::query!(
            r#"
            SELECT m.run_id, m.pull_request_number, m.pull_request_url, m.merged_at
            FROM factory_merge m
            WHERE m.repo_url = $1
              AND m.base_branch = $2
              AND m.merged_at > COALESCE(
                  (
                      SELECT h.last_green_at
                      FROM factory_main_hold h
                      WHERE h.repo_url = $1 AND h.base_branch = $2
                  ),
                  '-infinity'::timestamptz
              )
            ORDER BY m.merged_at DESC
            LIMIT $3
            "#,
            base.url,
            base.base_branch,
            i64::from(limit),
        )
        .fetch_all(&self.pool)
        .await?;

        let mut merges = rows
            .into_iter()
            .map(|row| {
                Ok(FactoryMerge {
                    run_id: RunId::from_uuid(row.run_id),
                    number: u64::try_from(row.pull_request_number)
                        .map_err(|_| PgRunRepositoryErr::NumberOutOfRange)?,
                    url: row.pull_request_url,
                    merged_at: row.merged_at,
                })
            })
            .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;
        merges.reverse();
        Ok(merges)
    }

    #[tracing::instrument(err, skip(self))]
    async fn claim_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
    ) -> Result<bool, Self::Err> {
        let claimed = sqlx::query!(
            r#"
            UPDATE factory_main_hold
            SET task_claimed_at = now()
            WHERE repo_url = $1 AND base_branch = $2
              AND held_since = $3 AND task_claimed_at IS NULL
            "#,
            base.url,
            base.base_branch,
            since,
        )
        .execute(&self.pool)
        .await?;
        Ok(claimed.rows_affected() == 1)
    }

    #[tracing::instrument(err, skip(self))]
    async fn release_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_main_hold
            SET task_claimed_at = NULL
            WHERE repo_url = $1 AND base_branch = $2
              AND held_since = $3 AND task_id IS NULL
            "#,
            base.url,
            base.base_branch,
            since,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn record_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
        task_id: Uuid,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_main_hold
            SET task_id = $4
            WHERE repo_url = $1 AND base_branch = $2
              AND held_since = $3 AND task_claimed_at IS NOT NULL
            "#,
            base.url,
            base.base_branch,
            since,
            task_id.to_string(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
