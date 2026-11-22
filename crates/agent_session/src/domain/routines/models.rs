//! Transport-independent routine commands and sanitized results.

use agent_runtime_protocol::domain::action::AgentActionId;
use bots::domain::models::BotId;
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};

use crate::domain::model::{
    AgentMcpServer, AgentSessionId, SessionBilling, SessionSource, SessionVisibility,
};
use crate::domain::session_command::SessionCommand;

/// The most bytes of an agent's reply [`super::RoutineSessions::reply`]
/// answers with: its end, which says where the agent stopped.
pub const MAX_REPLY_BYTES: usize = 16 * 1024;

/// A persona selected by a routine's validated user owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateRoutineSession {
    /// User whose authority and credentials execution spends.
    pub owner: MacroUserIdStr<'static>,
    /// Selected persona, not a runtime allowlist entry.
    pub bot_id: BotId,
    /// Omitted means the persona's current default at preparation time.
    pub model: Option<String>,
    /// Apps the routine itself requires. The owner must have each one
    /// connected; the session gets them on top of the persona's own servers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<AgentMcpServer>,
}

/// An authorized selection. Validation does not establish runtime availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatedRoutineSession {
    /// Whether this deployment provisions the runtime.
    pub managed: bool,
}

/// Create a fresh session without delivering any prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrepareRoutineSession {
    /// Selection reauthorized at execution time.
    pub selection: ValidateRoutineSession,
    /// Caller-generated UUIDv7, allocated before awaiting preparation.
    pub session_id: AgentSessionId,
    /// Who is starting the session: a routine for the scheduler, the user
    /// for a coding agent dispatched on their behalf. Older schedulers omit it.
    #[serde(default)]
    pub source: SessionSource,
    /// Whether a routine's run is listed, from the routine's setting. Older
    /// schedulers omit it along with `source`.
    #[serde(default)]
    pub visibility: SessionVisibility,
    /// What opened the session. Older schedulers omit it.
    #[serde(
        default,
        skip_serializing_if = "trigger_context::TriggerOrigin::is_unknown"
    )]
    pub origin: trigger_context::TriggerOrigin,
    /// Whether the session's in-process turns are billed to its owner. Older
    /// schedulers omit it and get `Billable`, as every session was.
    #[serde(default, skip_serializing_if = "SessionBilling::is_billable")]
    pub billing: SessionBilling,
    /// Instructions the session keeps for every turn, snapshotted when it
    /// opens. A background session puts them in its cached prompt prefix
    /// (see `agent_inmem`), so text every opener of its kind sends belongs
    /// here and what is particular to one request in the prompt. Older
    /// schedulers omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The repository the session works in, as the
    /// `https://github.com/owner/name` URL a coding session takes, when it
    /// works in one: its runtime checks the repository out for it, as for a
    /// coding session started with that repository. Older callers omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
}

/// A verified, persisted session ready for its first prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedRoutineSession {
    /// The caller's requested session id.
    pub session_id: AgentSessionId,
}

/// Identity checked on every subsequent operation; an id alone is not authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutineSessionAction {
    /// Claimed routine owner, checked against the immutable session owner.
    pub owner: MacroUserIdStr<'static>,
    /// Persona the routine selected, checked against the session.
    pub bot_id: BotId,
    /// Caller-generated UUIDv7 session id.
    pub session_id: AgentSessionId,
    /// Caller-generated UUIDv7 identifying one prompt: the initial one, or a
    /// follow-up's ([`super::RoutineSessions::follow_up`]).
    pub action_id: AgentActionId,
}

/// A prompt sent to a prepared session: its first, or a follow-up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptRoutineSession {
    /// Owner, persona, session, and the prompt's action identity.
    pub action: RoutineSessionAction,
    /// Routine guidance, instructions, and context; not persona system settings.
    pub prompt: String,
    /// What made the routine fire, rendered ahead of the prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<trigger_context::TriggerContext>,
    /// Whether the prompt comes from work its owner runs out of sight, as
    /// a software factory Step's do: it follows the session for no one, so
    /// no list shows the session for it. Older callers omit it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub out_of_sight: bool,
}

/// A command to run in a session's checkout, beside its agent, and how
/// it ended ([`super::RoutineSessions::exec`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecRoutineSession {
    /// Owner, persona and session, checked as for a prompt; the action id
    /// names the command in the caller's records and logs.
    pub action: RoutineSessionAction,
    /// What to run.
    pub command: SessionCommand,
}

/// Prompt admission is not completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutinePromptAccepted {
    /// The requested action id.
    pub action_id: AgentActionId,
    /// Whether admission queued the prompt rather than immediately sending it.
    pub queued: bool,
}

/// Content-free progress of the specific initial action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub enum RoutineActionStatus {
    /// No terminal evidence yet. The scheduler's execution deadline still applies.
    Pending(RoutinePendingReason),
    /// The initial turn explicitly ended normally.
    Succeeded,
    /// The initial turn explicitly ended unsuccessfully.
    Failed(RoutineFailureReason),
}

/// How much a session did, without what: for its opener's telemetry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutineActivity {
    /// Tool calls the agent made, across every turn.
    pub tool_calls: u32,
    /// Turns the session ran.
    pub turns: u32,
}

/// Why the action has not established completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutinePendingReason {
    /// The initial prompt has not appeared in authoritative history yet.
    AwaitingPrompt,
    /// The initial turn has no stop reason yet.
    Running,
    /// Human permission is required; routines never grant it themselves.
    Permission,
    /// Human input is required; routines never answer it themselves.
    Elicitation,
    /// The runtime disconnected without proving completion.
    Disconnected,
}

/// Closed, sanitized terminal outcomes. No transcript or runtime error text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutineFailureReason {
    /// Explicitly cancelled by the runtime.
    Cancelled,
    /// The agent declined the task.
    Refusal,
    /// Token budget exhausted.
    MaxTokens,
    /// Turn request budget exhausted.
    MaxTurnRequests,
    /// The runtime answered the prompt with an error.
    RuntimeError,
    /// The usage limit of the provider account the runtime works with (a
    /// subscription signed in on the runtime's machine, or a cloud agent's
    /// own account) ended the turn, as the runtime classified it. Any work
    /// the turn did before stays in the session.
    UsageLimit,
    /// An unrecognized stop reason is never success.
    UnknownStopReason,
}

/// Safe errors shared by internal adapters and the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum RoutineSessionError {
    /// Definitive refusal before Macro-funded work was accepted.
    #[cfg(feature = "admission")]
    #[error(transparent)]
    Admission(#[from] crate::domain::error::AiAdmissionError),
    /// Non-v7 execution ids, empty prompt, or blank supplied model.
    #[error("invalid routine session command")]
    InvalidCommand,
    /// Persona was deleted or is not an executable agent.
    #[error("selected agent is unavailable")]
    PersonaUnavailable,
    /// Persona access or session ownership was denied.
    #[error("routine owner is not authorized")]
    Forbidden,
    /// The owner has not connected an app the routine requires.
    #[error("connect every app this routine requires")]
    AppNotConnected,
    /// The selected agent's runtime opens its own sessions and cannot be
    /// handed the routine's apps.
    #[error("the selected agent cannot use apps added to a routine")]
    AppsUnsupported,
    /// A runtime could not serve the selected agent.
    #[error("selected runtime is unavailable")]
    RuntimeUnavailable,
    /// The selected agent's harness is an agent teammate's computer.
    #[error("{}", crate::domain::error::TEAM_AGENT_COMPUTER_MESSAGE)]
    TeamAgentComputer,
    /// Preparation returned another owner's, persona's, or session's row.
    #[error("prepared session does not match the request")]
    SessionMismatch,
    /// The requested model was not applied before prompting.
    #[error("prepared session does not use the requested model")]
    ModelMismatch,
    /// Session already exists or has accepted a first prompt.
    #[error("routine session has already been used")]
    Conflict,
    /// The session's runtime runs no command beside its agent: a paired
    /// machine, a cloud agent or the in-process agent.
    #[error("the session's runtime runs no command beside its agent")]
    CommandUnsupported,
    /// Prompt admission failed and may already have taken effect. Never replay it.
    #[error("prompt delivery is uncertain; do not retry")]
    PromptDeliveryUnknown,
    /// A domain port failed; adapters must not expose its underlying details.
    #[error("routine session operation failed")]
    OperationFailed,
}

impl RoutineSessionError {
    /// Whether the same command may succeed if sent again later, with
    /// nothing else changed: a runtime or port that was unavailable, or a
    /// billing check that could not run. A refusal, a mismatch or a conflict
    /// answers the same way again, and a prompt of unknown delivery must
    /// never be sent again.
    pub fn is_transient(&self) -> bool {
        match self {
            #[cfg(feature = "admission")]
            Self::Admission(error) => error.is_retryable(),
            Self::RuntimeUnavailable | Self::OperationFailed => true,
            Self::InvalidCommand
            | Self::PersonaUnavailable
            | Self::Forbidden
            | Self::AppNotConnected
            | Self::AppsUnsupported
            | Self::TeamAgentComputer
            | Self::SessionMismatch
            | Self::ModelMismatch
            | Self::Conflict
            | Self::CommandUnsupported
            | Self::PromptDeliveryUnknown => false,
        }
    }
}

#[cfg(test)]
mod test;
