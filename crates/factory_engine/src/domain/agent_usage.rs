//! What the agent sessions of Steps used, for Insights, quality, Benchmarks
//! and a Run's token budget: the usage a session's runtime reported, which
//! the factory keeps (`SessionAgent` keeps it as turns end and before it
//! deletes a session), else the model calls Macro's usage log recorded for
//! the session. A runtime that calls its model directly, as Claude Code does
//! through macrod, records nothing in the usage log; one that calls it
//! through Macro's model router records its calls there.

#[cfg(test)]
mod test;

use std::collections::HashSet;

use ai_usage::domain::session_usage::{SessionTokens, SessionUsageReader};
use factory_records::domain::session_usage::{SessionUsage, SessionUsageRepository};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

/// Reads each session's usage from what its runtime reported, kept in
/// `kept`, and the sessions with none kept from the usage log `log`.
#[derive(Debug, Clone)]
pub struct AgentUsageReader<U, R> {
    log: U,
    kept: R,
}

impl<U, R> AgentUsageReader<U, R> {
    /// Read kept usage from `kept`, and the rest from `log`.
    pub fn new(log: U, kept: R) -> Self {
        Self { log, kept }
    }
}

/// Errors reading agent sessions' usage.
#[derive(Debug, thiserror::Error)]
pub enum AgentUsageError {
    /// The usage the factory kept could not be read.
    #[error("the kept session usage could not be read: {0:?}")]
    Kept(rootcause::Report),
    /// The usage log could not be read.
    #[error("the usage log could not be read: {0:?}")]
    Log(rootcause::Report),
}

fn report(error: impl std::error::Error + Send + Sync + 'static) -> rootcause::Report {
    rootcause::Report::new(error).into_dynamic()
}

/// The tokens of a session's kept usage. A runtime that reported no cost
/// leaves what the session cost unknown: it counts as one call with no
/// price, so no money figure is given for what it is part of, as for a
/// model call with no price recorded.
fn tokens(usage: SessionUsage) -> SessionTokens {
    SessionTokens {
        session_id: usage.session_id,
        input: usage.input_tokens,
        output: usage.output_tokens,
        cache_read: usage.cache_read_tokens,
        cache_write: usage.cache_write_tokens,
        cost_usd: usage.cost_usd.unwrap_or(0.0),
        unpriced_calls: u64::from(usage.cost_usd.is_none()),
        reported_by_runtime: true,
    }
}

impl<U, R> SessionUsageReader for AgentUsageReader<U, R>
where
    U: SessionUsageReader,
    R: SessionUsageRepository,
{
    type Err = AgentUsageError;

    /// Each session counts once. One whose runtime reported its cost reads
    /// as reported. Otherwise, one the usage log recorded calls for reads
    /// from the log, whose calls are priced: a runtime on Macro's model
    /// router may report its tokens with no cost. Any other session with
    /// usage kept reads as its runtime reported it, its cost unknown.
    async fn session_tokens(
        &self,
        user: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<Vec<SessionTokens>, Self::Err> {
        let kept = self
            .kept
            .session_usage(user, sessions)
            .await
            .map_err(|error| AgentUsageError::Kept(report(error)))?;
        let costed: HashSet<Uuid> = kept
            .iter()
            .filter(|usage| usage.cost_usd.is_some())
            .map(|usage| usage.session_id)
            .collect();
        let rest: Vec<Uuid> = sessions
            .iter()
            .filter(|session| !costed.contains(session))
            .copied()
            .collect();
        let mut read = if rest.is_empty() {
            Vec::new()
        } else {
            self.log
                .session_tokens(user, &rest)
                .await
                .map_err(|error| AgentUsageError::Log(report(error)))?
        };
        let logged: HashSet<Uuid> = read.iter().map(|tokens| tokens.session_id).collect();
        read.extend(
            kept.into_iter()
                .filter(|usage| {
                    costed.contains(&usage.session_id) || !logged.contains(&usage.session_id)
                })
                .map(tokens),
        );
        Ok(read)
    }
}
