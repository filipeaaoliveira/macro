//! The Postgres Run repository's merge guard: the Run that holds a pull
//! request's merge, read from `factory_run`.

#[cfg(test)]
mod test;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::board::pull_request_key;
use crate::domain::merge_guard::MergeGuardRepository;
use crate::domain::models::RunId;

impl MergeGuardRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn live_run_of_pull_request(&self, page: &str) -> Result<Option<RunId>, Self::Err> {
        // Read through `factory_run_pull_request_url_idx`, which keys Runs
        // by their page in lower case. A Run has not ended while it has no
        // outcome.
        let id = sqlx::query_scalar!(
            r#"
            SELECT id FROM factory_run
            WHERE lower(pull_request_url) = $1 AND outcome IS NULL
            ORDER BY created_at DESC, id DESC
            LIMIT 1
            "#,
            pull_request_key(page),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(id.map(RunId::from_uuid))
    }
}
