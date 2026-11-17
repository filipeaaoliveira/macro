//! A Run, its Steps, and the names they carry.

#[cfg(test)]
mod test;

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The most bytes a Run's request may hold (the `factory_run.request` check).
pub const MAX_REQUEST_BYTES: usize = 65_536;

/// The most bytes a Step's output may hold (the `factory_step.output` check).
pub const MAX_OUTPUT_BYTES: usize = 65_536;

/// The most bytes a Gate's question may hold (the `factory_gate.question`
/// check).
pub const MAX_QUESTION_BYTES: usize = 4_096;

/// The most bytes the note on a Gate's answer may hold (the
/// `factory_gate.note` check).
pub const MAX_NOTE_BYTES: usize = 4_096;

/// A Run's id.
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
    utoipa::ToSchema,
)]
#[serde(transparent)]
#[schema(value_type = String, format = Uuid)]
pub struct RunId(Uuid);

impl RunId {
    /// Mint a new UUIDv7 id.
    pub fn generate() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an already persisted id.
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }

    /// The inner UUID.
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RunId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// A name a Flow gives something: lowercase letters, digits, `-` and `_`, at
/// most 64 of them.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{kind} {value:?} must be 1 to 64 lowercase letters, digits, - or _")]
pub struct InvalidName {
    kind: &'static str,
    value: String,
}

/// `value` if it is a valid name for a Flow's `kind` of thing (`"entry id"`,
/// `"outcome"`, ...), else why not.
pub fn checked_name(kind: &'static str, value: String) -> Result<String, InvalidName> {
    if lazy_regex::regex_is_match!(r"^[a-z0-9_-]{1,64}$", &value) {
        Ok(value)
    } else {
        Err(InvalidName { kind, value })
    }
}

/// The id of one of a Flow's entries. A Step is a visit of an entry.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String)]
pub struct EntryId(String);

impl EntryId {
    /// The id as written in the Flow.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EntryId {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("entry id", value).map(Self)
    }
}

impl From<EntryId> for String {
    fn from(id: EntryId) -> Self {
        id.0
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a Step ended, in the words of its Flow (`done`, `clean`, `blockers`,
/// ...). The Flow decides where each outcome takes the Run.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String)]
pub struct Outcome(String);

impl Outcome {
    /// The outcome of a Step that did its work.
    pub fn done() -> Self {
        Self("done".to_string())
    }

    /// The outcome of a Step that found nothing to change.
    pub fn unchanged() -> Self {
        Self("unchanged".to_string())
    }

    /// The outcome of a Step whose work failed after its retries. A Flow may
    /// route it like any other outcome.
    pub fn failed() -> Self {
        Self("failed".to_string())
    }

    /// The outcome as written in the Flow.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Outcome {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("outcome", value).map(Self)
    }
}

impl From<Outcome> for String {
    fn from(outcome: Outcome) -> Self {
        outcome.0
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A job in a Flow, such as `planner`, `writer`, `reviewer` or `fixer`. An
/// entry whose Step an agent does names the Role, and the factory's agent
/// definition lists the Agents that may fill it.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String)]
pub struct Role(String);

impl Role {
    /// The Role as written in the Flow.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Role {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("role", value).map(Self)
    }
}

impl From<Role> for String {
    fn from(role: Role) -> Self {
        role.0
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name the factory's agent definition gives an Agent: a harness, a
/// model and its instructions.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schema(value_type = String)]
pub struct AgentName(String);

impl AgentName {
    /// The name as the agent definition writes it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AgentName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("agent", value).map(Self)
    }
}

impl From<AgentName> for String {
    fn from(name: AgentName) -> Self {
        name.0
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a Run ended.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RunOutcome {
    /// The Flow reached its successful end.
    Success,
    /// The Flow ended for a person to take over.
    Manual,
    /// An entry ran out of visits and its Flow named nowhere else to go.
    Blocked,
    /// A Step failed with nowhere to go, or the Flow had no way on.
    Failed,
    /// A person cancelled the Run.
    Cancelled,
}

/// A Run that has ended: how, and when.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunEnd {
    /// How the Run ended.
    pub outcome: RunOutcome,
    /// When it ended.
    pub ended_at: DateTime<Utc>,
}

/// One Run of a Flow.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    /// The Run's id, also its Temporal workflow id.
    pub id: RunId,
    /// What the Run was asked to do.
    pub request: String,
    /// Who started the Run: its agent Steps run as them.
    pub user_id: MacroUserIdStr<'static>,
    /// When the Run was created.
    pub created_at: DateTime<Utc>,
    /// The Macro task the Run works on: its task document's id. None when
    /// the Run has no task, or its task was deleted.
    pub task_id: Option<Uuid>,
    /// The GitHub repository the Run works on; none for a Run that works on
    /// no code.
    pub repository: Option<Repository>,
    /// How and when it ended; none while it runs.
    pub end: Option<RunEnd>,
}

/// The GitHub repository a Run works on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Repository {
    /// Its `https://github.com/owner/name` URL.
    pub url: String,
    /// The branch the Run's work starts from and proposes its change to: the
    /// repository's default when the Run started.
    pub base_branch: String,
}

/// What keeping a new Run came to.
#[derive(Debug, Clone, PartialEq)]
pub enum RunInsert {
    /// The Run is kept.
    Inserted(Run),
    /// No document has the task id the Run was to work on, so nothing was
    /// kept.
    NoSuchTask,
}

/// Whether a Run was linked to a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLink {
    /// The Run now works on the task.
    Linked,
    /// No Run has this id.
    NoSuchRun,
    /// No document has the task's id.
    NoSuchTask,
}

/// A Step that has ended: its outcome, what it produced, and when.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StepEnd {
    /// How the Step ended.
    pub outcome: Outcome,
    /// What the Step produced, for a person to read.
    pub output: String,
    /// When it ended.
    pub ended_at: DateTime<Utc>,
}

/// A person's answer to a Gate.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GateAnswer {
    /// The answer, one the Gate offered: the outcome the Flow routes.
    pub answer: Outcome,
    /// What the person added, for the Run's later Steps to read.
    pub note: Option<String>,
    /// Who answered; none once their account is gone.
    pub user_id: Option<MacroUserIdStr<'static>>,
    /// When they answered.
    pub answered_at: DateTime<Utc>,
}

/// A question a Step's end puts to a person. The Run waits for the answer.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    /// The question.
    pub question: String,
    /// The answers it offers, in the Flow's order.
    pub answers: Vec<Outcome>,
    /// When the Run started waiting.
    pub opened_at: DateTime<Utc>,
    /// The person's answer; none while the Run waits.
    pub answer: Option<GateAnswer>,
}

/// The most Steps a Run takes; past it the Run ends `blocked`, so a Flow
/// that loops cannot run forever.
pub const MAX_STEPS: u16 = 200;

/// Which of a Run's Steps: its place among them, from 1. A Step keeps its
/// place even when a reset numbers its entry's visits from 1 again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StepKey {
    /// The Run.
    pub run_id: RunId,
    /// The Step's place in it, from 1.
    pub seq: u16,
}

/// One visit of one of a Run's entries.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// The Step's place among the Run's Steps, from 1.
    pub seq: u16,
    /// The entry the Step visits.
    pub entry: EntryId,
    /// Which visit of the entry it is, from 1.
    pub visit: u16,
    /// When the Step started.
    pub started_at: DateTime<Utc>,
    /// How and when it ended; none while it runs.
    pub end: Option<StepEnd>,
    /// The question its end put to a person, if its entry has a Gate.
    pub gate: Option<Gate>,
    /// The Role an agent filled for the Step; none for a Step no agent does.
    pub role: Option<Role>,
    /// The Agent that took the Step, from its Role's Agents.
    pub agent: Option<AgentName>,
    /// The Agents before it that could not start the Step, in order, and
    /// why.
    pub passed_over: Vec<PassedOver>,
}

/// An Agent that could not start a Step, so the next Agent of its Role took
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PassedOver {
    /// The Agent.
    pub agent: AgentName,
    /// Why it could not start, as a code such as `ai_allowance_exhausted`.
    pub reason: String,
}

/// The agent session a Step works in: the session, the action of the
/// prompt that starts the Step's work, both UUIDv7s the factory makes, and
/// the Agent whose session it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepSession {
    /// The agent session.
    pub session_id: Uuid,
    /// Its first prompt's action.
    pub action_id: Uuid,
    /// The Agent the session is of.
    pub agent: AgentName,
}

/// A Step's session as `RunRepository::claim_session` left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedSession {
    /// The session the Step works in.
    pub session: StepSession,
    /// Whether this claim recorded it; false when an earlier attempt of the
    /// Step already had.
    pub new: bool,
}

/// The session a Step claimed, as a stop sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoppingSession {
    /// The session.
    pub session: StepSession,
    /// Whether an attempt saw it open.
    pub opened: bool,
    /// Whether a prepare sent for it may still open it: one was sent, and
    /// less long ago than the bound the stop gave.
    pub may_still_open: bool,
}

/// A Step as the agent session it works in finds it.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionStep {
    /// The Step.
    pub key: StepKey,
    /// The Run's owner, as whom the session runs.
    pub owner: MacroUserIdStr<'static>,
    /// The outcomes the agent may hand back.
    pub offered: Vec<Outcome>,
    /// Whether the Step has ended.
    pub ended: bool,
}

/// What an agent handed back for its Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandBack {
    /// One of the outcomes the Step offered.
    pub outcome: Outcome,
    /// What the agent produced, for a person and the Run's later Steps.
    pub output: String,
}
