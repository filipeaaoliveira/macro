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

/// Reads the tokens recorded for agent sessions from `ai_usage`, and what
/// the priced calls cost.
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

        // Driven from the session ids: each session's calls are looked up
        // through `ai_usage_agent_session_id_idx`, so the read is bounded by
        // the sessions asked for and their calls, whatever the planner
        // believes about the table. The user check keeps a caller to usage
        // that ran as them; it is fenced out of the lookup (`OFFSET 0`), or a
        // planner with stale statistics adds the user's own index to every
        // lookup and reads their whole history once per session. A session
        // makes one row per model call, so the timeout bounds a long one.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let rows = sqlx::query!(
            r#"
            SELECT
                sessions.id AS "session_id!",
                sum(calls.input_tokens)::bigint AS "input!",
                sum(calls.output_tokens)::bigint AS "output!",
                sum(calls.cache_read_input_tokens)::bigint AS "cache_read!",
                sum(calls.cache_write_input_tokens)::bigint AS "cache_write!",
                coalesce(sum(calls.total::float8), 0)::float8 AS "cost_usd!",
                count(*) FILTER (WHERE calls.total IS NULL) AS "unpriced_calls!"
            FROM unnest($1::uuid[]) AS sessions(id)
            CROSS JOIN LATERAL (
                SELECT user_id, input_tokens, output_tokens, cache_read_input_tokens,
                       cache_write_input_tokens, total
                FROM ai_usage
                WHERE agent_session_id = sessions.id
                OFFSET 0
            ) calls
            WHERE calls.user_id = $2
            GROUP BY sessions.id
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
                cost_usd: row.cost_usd,
                unpriced_calls: tokens(row.unpriced_calls),
                reported_by_runtime: false,
            })
            .collect())
    }
}
