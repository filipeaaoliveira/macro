//! PostgreSQL implementation of the [`UserDirectory`] port.

#[cfg(test)]
mod test;

use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use crate::domain::ports::UserDirectory;

/// Reads Macro's accounts from MacroDB.
#[derive(Debug, Clone)]
pub struct PgUserDirectory {
    pool: PgPool,
}

impl PgUserDirectory {
    /// Create a directory backed by the provided pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl UserDirectory for PgUserDirectory {
    type Err = sqlx::Error;

    #[tracing::instrument(err, skip_all)]
    async fn user_exists(&self, user_id: &MacroUserIdStr<'static>) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM "User" WHERE id = $1) AS "exists!""#,
            user_id.as_ref(),
        )
        .fetch_one(&self.pool)
        .await
    }
}
