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
    /// When the Run was created.
    pub created_at: DateTime<Utc>,
    /// How and when it ended; none while it runs.
    pub end: Option<RunEnd>,
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

/// One visit of one of a Run's entries.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Step {
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
}
