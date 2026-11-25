//! What agent sessions used, summed per session: the model tokens their
//! calls recorded with the session's id, and what the priced ones cost.

use macro_user_id::user_id::MacroUserIdStr;
use macro_uuid::Uuid;

/// The tokens the model calls of one agent session used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionTokens {
    /// The agent session.
    pub session_id: Uuid,
    /// Input tokens, besides those read from or written to the prompt cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write: u64,
    /// What its priced calls cost, in US dollars.
    pub cost_usd: f64,
    /// How many of its calls have no price, as a model with no price
    /// recorded when they were made: their cost is not in `cost_usd`.
    pub unpriced_calls: u64,
    /// Whether the session's runtime reported this itself, rather than the
    /// usage log recording each model call: a runtime that calls its model
    /// directly. Such a runtime's session with nothing reported is not
    /// known to have made no call.
    pub reported_by_runtime: bool,
    /// What its calls billed to an agent's own API key used, when any
    /// were: a session runs as one agent, so on one key at most.
    pub api_key: Option<ApiKeySpend>,
}

/// What an agent session's model calls billed to an agent's own API key
/// (`ai_usage.agent_api_key_id`) cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ApiKeySpend {
    /// The key, by the agent that holds it (`agent_api_keys.bot_id`).
    pub key: Uuid,
    /// What its priced calls on the key cost, in US dollars.
    pub cost_usd: f64,
    /// How many of its calls on the key have no price.
    pub unpriced_calls: u64,
}

/// Reads the usage recorded for agent sessions.
pub trait SessionUsageReader: Send + Sync + 'static {
    /// The error type returned by reads.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The tokens recorded for each of `sessions` that ran as `user`, from
    /// its turns' calls: Macro's own calls about the session (its title, its
    /// activity summary) are left out. A session with no turn recorded for
    /// `user` has no entry: one whose model calls were not made through
    /// Macro, or one of someone else's.
    fn session_tokens(
        &self,
        user: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> impl Future<Output = Result<Vec<SessionTokens>, Self::Err>> + Send;
}
