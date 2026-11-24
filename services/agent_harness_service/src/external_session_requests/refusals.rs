//! Refusals of requested sessions: a runtime that will not open a session
//! it was asked for says so, and why, through
//! `POST /agent-sessions/{session_id}/refusal`. The refusal is kept a short
//! while, in Redis so every replica sees it, and the requester waiting on
//! the session reads it there and stops waiting.

use std::time::Duration;

use agent_session::domain::model::AgentSessionId;
use agent_session::domain::ports::BotDirectory;
use agent_session::inbound::axum_router::RuntimeRefusalRequest;
use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bot_id::BotId;
use macro_authorization::{
    HarnessOnly, MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState,
};
use redis::AsyncCommands as _;
use serde::{Deserialize, Serialize};

/// How long a refusal is kept: past the longest a requester waits on its
/// session.
pub(crate) const REFUSAL_KEPT: Duration = Duration::from_secs(10 * 60);

/// The most bytes of a refusal's reason that are kept and shown.
pub(crate) const MAX_REASON_BYTES: usize = 1024;

/// A runtime's refusal to open a requested session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// The agent the session was requested for.
    pub bot_id: BotId,
    /// Why the runtime opens no such session, for the session's owner.
    pub reason: String,
}

/// Where runtimes' refusals of requested sessions are kept.
pub trait SessionRefusals: Send + Sync + 'static {
    /// Keep `refusal` of `session` for [`REFUSAL_KEPT`].
    fn record(
        &self,
        session: AgentSessionId,
        refusal: Refusal,
    ) -> impl Future<Output = anyhow::Result<()>> + Send;

    /// The refusal of `session`, if a runtime sent one lately.
    fn refusal(
        &self,
        session: AgentSessionId,
    ) -> impl Future<Output = anyhow::Result<Option<Refusal>>> + Send;
}

/// No refusals: a requester over this waits on its session until its
/// timeout, as before runtimes could refuse.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRefusals;

impl SessionRefusals for NoRefusals {
    async fn record(&self, _session: AgentSessionId, _refusal: Refusal) -> anyhow::Result<()> {
        Ok(())
    }

    async fn refusal(&self, _session: AgentSessionId) -> anyhow::Result<Option<Refusal>> {
        Ok(None)
    }
}

/// Refusals in Redis, one key per session, gone after [`REFUSAL_KEPT`].
#[derive(Clone)]
pub struct RedisSessionRefusals {
    redis: redis::Client,
}

impl RedisSessionRefusals {
    /// Refusals kept in `redis`.
    pub fn new(redis: redis::Client) -> Self {
        Self { redis }
    }
}

fn key(session: AgentSessionId) -> String {
    format!("agent-harness:session-refusal:{session}")
}

impl SessionRefusals for RedisSessionRefusals {
    async fn record(&self, session: AgentSessionId, refusal: Refusal) -> anyhow::Result<()> {
        let payload = serde_json::to_string(&refusal)?;
        let mut connection = self.redis.get_multiplexed_async_connection().await?;
        connection
            .set_ex::<_, _, ()>(key(session), payload, REFUSAL_KEPT.as_secs())
            .await?;
        Ok(())
    }

    async fn refusal(&self, session: AgentSessionId) -> anyhow::Result<Option<Refusal>> {
        let mut connection = self.redis.get_multiplexed_async_connection().await?;
        let payload: Option<String> = connection.get(key(session)).await?;
        payload
            .map(|payload| serde_json::from_str(&payload).map_err(anyhow::Error::from))
            .transpose()
    }
}

/// `reason`, trimmed, and cut to at most [`MAX_REASON_BYTES`] between two
/// characters.
pub(crate) fn bounded_reason(reason: &str) -> String {
    let reason = reason.trim();
    let mut end = reason.len().min(MAX_REASON_BYTES);
    while !reason.is_char_boundary(end) {
        end -= 1;
    }
    reason[..end].to_owned()
}

/// What the refusal route needs: who the agents' harnesses are, where
/// refusals are kept, and the harness token check.
pub struct RefusalsState<Bots, Refusals, Auth> {
    bots: std::sync::Arc<Bots>,
    refusals: Refusals,
    authorization: MacroAuthorizationState<Auth>,
}

impl<Bots, Refusals, Auth> RefusalsState<Bots, Refusals, Auth> {
    /// The route's state.
    pub fn new(
        bots: std::sync::Arc<Bots>,
        refusals: Refusals,
        authorization: MacroAuthorizationState<Auth>,
    ) -> Self {
        Self {
            bots,
            refusals,
            authorization,
        }
    }
}

impl<Bots, Refusals: Clone, Auth> Clone for RefusalsState<Bots, Refusals, Auth> {
    fn clone(&self) -> Self {
        Self {
            bots: self.bots.clone(),
            refusals: self.refusals.clone(),
            authorization: self.authorization.clone(),
        }
    }
}

impl<Bots, Refusals, Auth> FromRef<RefusalsState<Bots, Refusals, Auth>>
    for MacroAuthorizationState<Auth>
{
    fn from_ref(state: &RefusalsState<Bots, Refusals, Auth>) -> Self {
        state.authorization.clone()
    }
}

/// `POST /agent-sessions/{session_id}/refusal`, for runtimes' daemons only:
/// not part of the browser API, so not in its OpenAPI document.
pub fn router<Bots, Refusals, Auth>(state: RefusalsState<Bots, Refusals, Auth>) -> Router
where
    Bots: BotDirectory,
    Refusals: SessionRefusals + Clone,
    Auth: MacroAuthorizationService,
{
    Router::new()
        .route(
            "/agent-sessions/{session_id}/refusal",
            post(refuse::<Bots, Refusals, Auth>),
        )
        .with_state(state)
}

/// Keep a runtime's refusal of a session it was asked to open. Only the
/// harness the session's agent is bound to may refuse it.
async fn refuse<Bots, Refusals, Auth>(
    State(state): State<RefusalsState<Bots, Refusals, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, HarnessOnly>,
    Path(session_id): Path<AgentSessionId>,
    Json(request): Json<RuntimeRefusalRequest>,
) -> Response
where
    Bots: BotDirectory,
    Refusals: SessionRefusals + Clone,
    Auth: MacroAuthorizationService,
{
    let bot_id = BotId::new_from_uuid(request.bot_id);
    let harness = match state.bots.bot_facts(bot_id).await {
        Ok(facts) => facts.and_then(|facts| facts.harness_id),
        Err(error) => {
            tracing::error!(?error, %session_id, "a session refusal's agent could not be read");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    if harness != Some(caller.authorization.harness_id) {
        return (
            StatusCode::FORBIDDEN,
            "this agent is not bound to this harness",
        )
            .into_response();
    }
    let reason = bounded_reason(&request.reason);
    if reason.is_empty() {
        return (StatusCode::BAD_REQUEST, "a refusal says why").into_response();
    }
    tracing::info!(%session_id, %bot_id, reason, "a runtime refused a requested session");
    match state
        .refusals
        .record(session_id, Refusal { bot_id, reason })
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::error!(?error, %session_id, "a session refusal could not be kept");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
