//! The Postgres Run repository's agent session usage: `factory_session_usage`.

#[cfg(test)]
mod test;

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::session_usage::{SessionUsage, SessionUsageRepository};

/// A token count as the table keeps it: past `i64::MAX` it stays there.
fn stored(count: u64) -> i64 {
    i64::try_from(count).unwrap_or(i64::MAX)
}

/// A stored token count, which the table keeps at zero or more.
fn count(stored: i64) -> u64 {
    u64::try_from(stored).unwrap_or(0)
}

impl SessionUsageRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, usage), fields(session_id = %usage.session_id))]
    async fn record_session_usage(&self, usage: &SessionUsage) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_session_usage (
                session_id, user_id, bot_id, input_tokens, output_tokens,
                cache_read_tokens, cache_write_tokens, cost_usd, model
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (session_id) DO UPDATE SET
                input_tokens = GREATEST(factory_session_usage.input_tokens, EXCLUDED.input_tokens),
                output_tokens = GREATEST(factory_session_usage.output_tokens, EXCLUDED.output_tokens),
                cache_read_tokens = GREATEST(
                    factory_session_usage.cache_read_tokens, EXCLUDED.cache_read_tokens
                ),
                cache_write_tokens = GREATEST(
                    factory_session_usage.cache_write_tokens, EXCLUDED.cache_write_tokens
                ),
                cost_usd = GREATEST(factory_session_usage.cost_usd, EXCLUDED.cost_usd),
                model = COALESCE(EXCLUDED.model, factory_session_usage.model),
                recorded_at = now()
            WHERE factory_session_usage.user_id = EXCLUDED.user_id
            "#,
            usage.session_id,
            usage.user_id.as_ref(),
            usage.bot_id,
            stored(usage.input_tokens),
            stored(usage.output_tokens),
            stored(usage.cache_read_tokens),
            stored(usage.cache_write_tokens),
            usage.cost_usd.filter(|cost| cost.is_finite() && *cost >= 0.0),
            usage.model.as_deref().filter(|model| !model.is_empty()),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, user, sessions), fields(sessions = sessions.len()))]
    async fn session_usage(
        &self,
        user: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<Vec<SessionUsage>, Self::Err> {
        if sessions.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query!(
            r#"
            SELECT session_id, user_id, bot_id, input_tokens, output_tokens,
                cache_read_tokens, cache_write_tokens, cost_usd, model
            FROM factory_session_usage
            WHERE session_id = ANY($1) AND user_id = $2
            "#,
            sessions,
            user.as_ref(),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(SessionUsage {
                    session_id: row.session_id,
                    user_id: MacroUserIdStr::try_from(row.user_id.clone())
                        .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.user_id))?,
                    bot_id: row.bot_id,
                    input_tokens: count(row.input_tokens),
                    output_tokens: count(row.output_tokens),
                    cache_read_tokens: count(row.cache_read_tokens),
                    cache_write_tokens: count(row.cache_write_tokens),
                    cost_usd: row.cost_usd,
                    model: row.model,
                })
            })
            .collect()
    }
}
