//! What the runtime of a Step's agent session reported its model calls
//! used, kept by the factory because the session's log goes with the
//! session. A runtime that calls its model directly records nothing in
//! Macro's own usage log, so this is what Insights, quality and the token
//! budget read for its sessions.

use std::future::Future;

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

/// One agent session's usage, as its runtime reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionUsage {
    /// The agent session.
    pub session_id: Uuid,
    /// Who the session ran as: the Run's owner.
    pub user_id: MacroUserIdStr<'static>,
    /// The bot that ran it.
    pub bot_id: Uuid,
    /// Input tokens, besides those read from or written to the prompt cache.
    pub input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read_tokens: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write_tokens: u64,
    /// What the session cost, in US dollars; none when its runtime reported
    /// no cost.
    pub cost_usd: Option<f64>,
    /// The model that used the most tokens, when the runtime named it.
    pub model: Option<String>,
}

/// Keeps and reads the usage agent sessions' runtimes reported.
pub trait SessionUsageRepository: Send + Sync + 'static {
    /// The error type returned by reads and writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Keep `usage` for its session. A session's log only grows, so each
    /// count keeps the larger of what was kept and what is given, and a
    /// cost once kept stays when a later read has none: a read of a log
    /// cut short never lowers what was kept.
    fn record_session_usage(
        &self,
        usage: &SessionUsage,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The usage kept for each of `sessions` that ran as `user`. A session
    /// with none kept for `user` has no entry.
    fn session_usage(
        &self,
        user: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> impl Future<Output = Result<Vec<SessionUsage>, Self::Err>> + Send;
}
