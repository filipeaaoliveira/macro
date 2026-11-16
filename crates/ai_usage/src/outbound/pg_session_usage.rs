//! Postgres-backed [`SessionUsageReader`] over the `ai_usage` table.

#[cfg(test)]
mod test;

use macro_user_id::user_id::MacroUserIdStr;
use macro_uuid::Uuid;
use sqlx::PgPool;

use crate::domain::session_usage::{SessionTokens, SessionUsageReader};

/// The most sessions one read sums up. Callers read at most this many at a
/// time.
pub const MAX_SESSIONS: usize = 10_000;

/// Reads the tokens recorded for agent sessions from `ai_usage`.
#[derive(Debug, Clone)]
pub struct PgSessionUsageReader {
    pool: PgPool,
}

impl PgSessionUsageReader {
    /// Create a reader over a connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Errors reading session usage.
#[derive(Debug, thiserror::Error)]
pub enum SessionUsageError {
    /// Underlying database error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// More sessions than one read sums up.
    #[error("{0} sessions are more than one read sums up ({MAX_SESSIONS})")]
    TooManySessions(usize),
}

/// A stored token count, which the table keeps at zero or more.
fn tokens(stored: i64) -> u64 {
    u64::try_from(stored).unwrap_or(0)
}

impl SessionUsageReader for PgSessionUsageReader {
    type Err = SessionUsageError;

    #[tracing::instrument(err, skip(self, user, sessions), fields(sessions = sessions.len()))]
    async fn session_tokens(
        &self,
        user: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<Vec<SessionTokens>, Self::Err> {
        if sessions.len() > MAX_SESSIONS {
            return Err(SessionUsageError::TooManySessions(sessions.len()));
        }
        if sessions.is_empty() {
            return Ok(Vec::new());
        }

        // `ai_usage_agent_session_id_idx` finds each session's rows, fenced
        // so the planner does not also read the person's whole history
        // through their own index; the user check then keeps a caller to
        // usage that ran as them. A session makes one row per model call, so
        // the timeout bounds a long one.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let rows = sqlx::query!(
            r#"
            WITH calls AS MATERIALIZED (
                SELECT agent_session_id, user_id, input_tokens, output_tokens,
                       cache_read_input_tokens, cache_write_input_tokens
                FROM ai_usage
                WHERE agent_session_id = ANY($1)
            )
            SELECT
                agent_session_id AS "session_id!",
                sum(input_tokens)::bigint AS "input!",
                sum(output_tokens)::bigint AS "output!",
                sum(cache_read_input_tokens)::bigint AS "cache_read!",
                sum(cache_write_input_tokens)::bigint AS "cache_write!"
            FROM calls
            WHERE user_id = $2
            GROUP BY agent_session_id
            "#,
            sessions,
            user.as_ref(),
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;

        Ok(rows
            .into_iter()
            .map(|row| SessionTokens {
                session_id: row.session_id,
                input: tokens(row.input),
                output: tokens(row.output),
                cache_read: tokens(row.cache_read),
                cache_write: tokens(row.cache_write),
            })
            .collect())
    }
}
