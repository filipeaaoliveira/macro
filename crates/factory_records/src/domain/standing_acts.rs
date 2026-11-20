//! Standing acts: what an agent working for a Run's owner did to the Run
//! from its code (start it, cancel it, link it to a task, answer its plan
//! or Blockers Gate) under a Standing Role of the Run's Factory Definition.
//! Each keeps the bot that acted (the agent session's, or the one a
//! sessionless call is signed for), its Agent, and the Standing Role that
//! allowed it.

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::{AgentName, InvalidName, RunId, checked_name};

/// An act a Standing Role may allow. The set is fixed: none merges, and a
/// Factory Definition cannot add one.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum StandingAct {
    /// Start a Run.
    Start,
    /// Cancel a Run.
    Cancel,
    /// Link a Run to a task.
    Link,
    /// Answer a Run's plan Gate.
    AnswerPlan,
    /// Answer a Run's Blockers Gate.
    AnswerBlockers,
}

/// The name a Factory Definition gives a Standing Role, such as `intake`,
/// `gatekeeper` or `operator`.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String)]
pub struct StandingRoleName(String);

impl StandingRoleName {
    /// The name as the Definition writes it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StandingRoleName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("standing role", value).map(Self)
    }
}

impl From<StandingRoleName> for String {
    fn from(name: StandingRoleName) -> Self {
        name.0
    }
}

impl fmt::Display for StandingRoleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Who did a standing act: the agent session it came from, if any, the
/// bot that acted, the Agent of the Definition that bot is, and the
/// Standing Role that allowed the act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandingActor {
    /// The agent session the call came from; none for a bot's code that
    /// runs in no agent session (its credential is signed for the bot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    /// The bot that acted: the session's, or the one the credential is
    /// signed for.
    pub bot_id: Uuid,
    /// The Agent the Definition names for that bot.
    pub agent: AgentName,
    /// The Standing Role that allowed the act.
    pub standing_role: StandingRoleName,
}

/// A standing act a Run keeps, as the Run API shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StandingActRecord {
    /// What was done.
    pub act: StandingAct,
    /// The Step whose Gate was answered, for an answer; none otherwise.
    pub seq: Option<u16>,
    /// The bot that did it.
    pub bot_id: Uuid,
    /// The Agent the Run's Definition names for that bot.
    pub agent: AgentName,
    /// The Standing Role that allowed it.
    pub standing_role: StandingRoleName,
    /// The agent session it came from; none for a bot's code that ran in
    /// no agent session.
    pub session_id: Option<Uuid>,
    /// When it was recorded.
    pub at: DateTime<Utc>,
}

/// Reads the bot behind an agent session, and keeps and reads Runs'
/// standing acts.
pub trait StandingActRepository: Send + Sync + 'static {
    /// The error type returned by reads and writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The bot that runs the agent session `session_id`, when `owner`
    /// owns the session; none otherwise.
    fn session_bot(
        &self,
        session_id: Uuid,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<Option<Uuid>, Self::Err>> + Send;

    /// Keep that `actor` did `act` to the Run `run_id` (to the Gate of its
    /// Step `seq`, for an answer). A Run keeps one start, one cancel and
    /// one answer per Gate: doing it again keeps the first. Each link is
    /// kept.
    fn record_standing_act(
        &self,
        run_id: RunId,
        act: StandingAct,
        seq: Option<u16>,
        actor: &StandingActor,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The standing acts of the Runs `ids`, each Run's in the order they
    /// were kept. A Run with none is left out.
    fn runs_standing_acts(
        &self,
        ids: &[RunId],
    ) -> impl Future<Output = Result<BTreeMap<RunId, Vec<StandingActRecord>>, Self::Err>> + Send;
}
