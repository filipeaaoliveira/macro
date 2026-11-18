//! A Run, its Steps, and the names they carry.

#[cfg(test)]
mod test;

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::automations::{AutomationStart, RunAutomation};
use crate::domain::standing_rules::{RunRules, StandingRule};

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

    /// The outcome of an agent Step that asks the Run's owner a question it
    /// cannot go on without: the Step ends with the question as its output,
    /// and a question Gate waits for the owner's answer. The Flow never
    /// routes it: the Step's Role goes on with the answer.
    pub fn question() -> Self {
        Self("question".to_string())
    }

    /// The one answer a question Gate offers: the owner's words go in its
    /// note.
    pub fn answer() -> Self {
        Self("answer".to_string())
    }

    /// The outcome of an agent Step whose agent stopped without handing back
    /// a result, even once reminded: the Step ends with the agent's last
    /// message as its output, and a Gate asks the Run's owner to `retry` or
    /// `stop`. The Flow never routes it.
    pub fn no_result() -> Self {
        Self("no_result".to_string())
    }

    /// The answer to a `no_result` Step's Gate that has its Role go on in
    /// the same session.
    pub fn retry() -> Self {
        Self("retry".to_string())
    }

    /// The answer to a `no_result` Step's Gate that fails the Step, for its
    /// Flow to route `failed`.
    pub fn stop() -> Self {
        Self("stop".to_string())
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
    /// Whether its owner created it in their own signed-in session, from an
    /// account that is not an agent's: only then does its request ask for
    /// the Control Files it names.
    pub created_in_person: bool,
    /// The Control File patterns it is checked against, as its owner's team
    /// had them when it was created; none for a Run created before Runs
    /// kept them, which the factory's default checks.
    #[serde(skip)]
    pub control_file_patterns: Option<Vec<String>>,
    /// The team those patterns came from; none when they are the default.
    #[serde(skip)]
    pub control_files_team_id: Option<Uuid>,
    /// The Control Files its branch changed, as the pull request Steps last
    /// read them. None when it changed none.
    pub control_files: Option<ControlFileChanges>,
    /// The Standing Rules that answer its Gates, as they were when it was
    /// created; none for a Run created before Runs kept them, whose Gates
    /// all wait for a person.
    pub standing_rules: Option<RunRules>,
    /// The Automation that started it, as it was then; none for a Run a
    /// person started.
    pub automation: Option<RunAutomation>,
    /// How and when it ended; none while it runs.
    pub end: Option<RunEnd>,
}

/// The Control Files a Run's branch changed: files that decide whether the
/// repository's checks pass, such as its tests, lint configuration and CI
/// workflows. The Run is Tampered when it changed one its request does not
/// ask for, and merges only once a person approves them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ControlFileChanges {
    /// The files its request does not ask for, from the repository's root,
    /// in GitHub's order: empty when `unlisted`.
    pub tampered: Vec<String>,
    /// The files its request asks for by naming them, which need no
    /// approval: empty when `unlisted`.
    pub named: Vec<String>,
    /// GitHub could not list all of the branch's changes, or they were more
    /// than a Run keeps (300 a list), so which Control Files it changed is
    /// not known.
    pub unlisted: bool,
}

impl ControlFileChanges {
    /// The most files either list keeps; a longer one is `unlisted`.
    pub const MAX_FILES: usize = 300;

    /// Whether the Run is Tampered: it changed a Control File its request
    /// does not ask for, or which ones it changed is not known.
    pub fn is_tampered(&self) -> bool {
        self.unlisted || !self.tampered.is_empty()
    }

    /// Whether `approved`, the Tampered files a person approved, covers
    /// them: every one, or, when the changes are `unlisted`, an approval
    /// given while they were.
    pub fn approved_by(&self, approved: &[String]) -> bool {
        if self.unlisted {
            return approved.is_empty();
        }
        self.tampered.iter().all(|file| approved.contains(file))
    }
}

/// The team a person is on, as the factory's settings read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamMembership {
    /// The team.
    pub team_id: Uuid,
    /// Whether they are its owner or one of its admins, from an account
    /// that is not an agent's.
    pub admin: bool,
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

/// A Run to keep before it starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NewRun<'a> {
    /// Its id, also its Temporal workflow id.
    pub id: RunId,
    /// The built-in Flow it walks, by name.
    pub flow: &'a str,
    /// What it is asked to do.
    pub request: &'a str,
    /// Who owns it.
    pub user_id: &'a MacroUserIdStr<'static>,
    /// The task document it works on, if any.
    pub task_id: Option<Uuid>,
    /// The repository it works in, if any.
    pub repository: Option<&'a Repository>,
    /// Whether its owner created it in their own signed-in session.
    pub created_in_person: bool,
    /// The Control File patterns it is checked against.
    pub control_file_patterns: &'a [String],
    /// The team those patterns came from; none when they are the default.
    pub control_files_team_id: Option<Uuid>,
    /// The Standing Rules that answer its Gates. Their Queue limit is not
    /// kept: a start is held to it by `insert_run_in_queue` ([`QueueHold`]).
    pub standing_rules: RunRules,
    /// The Automation that starts it, if one does: then no other Run an
    /// Automation started may be running on its task.
    pub automation: Option<AutomationStart<'a>>,
}

/// What keeping a new Run came to.
#[derive(Debug, Clone, PartialEq)]
pub enum RunInsert {
    /// The Run is kept.
    Inserted(Box<Run>),
    /// No document has the task id the Run was to work on, so nothing was
    /// kept.
    NoSuchTask,
    /// An Automation started it, and a Run another Automation started is
    /// running on its task, so nothing was kept.
    TaskBusy,
    /// A Run of the same Automation's firing is kept already.
    FiringStarted,
    /// Its owner already held as many places in their Queue as the Queue
    /// limit it started under, so nothing was kept: the places they held,
    /// counted up to the limit.
    QueueFull(QueuePlaces),
}

/// The places one owner holds in their Queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueuePlaces {
    /// Their open Runs that wait on them in person at a Gate or a question,
    /// leaving out a Gate the stand-in answered while they may override it.
    pub waiting: u16,
    /// Their open Runs of a Flow with a Gate of its own that have not
    /// reached a Gate yet: each holds the place it will ask for.
    pub starting: u16,
}

impl QueuePlaces {
    /// The places held in all.
    pub fn held(self) -> u16 {
        self.waiting.saturating_add(self.starting)
    }
}

/// How a new Run is held to its owner's Queue.
#[derive(Debug, Clone, Copy)]
pub struct QueueHold<'a> {
    /// The Queue limit of the rules it starts under: it is kept only while
    /// its owner holds fewer places than this before it, so they hold at
    /// most this many. Zero starts no Run; None sets no limit.
    pub limit: Option<u16>,
    /// The Flows whose Runs hold a place before their first Gate: those
    /// with a Gate of their own. A Run of another Flow holds one only once
    /// it asks something.
    pub asking_flows: &'a [&'a str],
}

impl QueueHold<'_> {
    /// No limit: the start only waits its turn behind its owner's others.
    pub const UNLIMITED: QueueHold<'static> = QueueHold {
        limit: None,
        asking_flows: &[],
    };
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

/// The answer to a Gate: a person's, a Standing Rule's (a Rule Answer), or
/// the stand-in's for the Run's owner.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GateAnswer {
    /// The answer, one the Gate offered: the outcome the Flow routes.
    pub answer: Outcome,
    /// What the person added, for the Run's later Steps to read.
    pub note: Option<String>,
    /// Who answered; none once their account is gone, and for a Rule
    /// Answer.
    pub user_id: Option<MacroUserIdStr<'static>>,
    /// The Standing Rule that answered, for a Rule Answer; none for a
    /// person's.
    pub rule: Option<StandingRule>,
    /// The Run's Tampered files the person approved with their answer: an
    /// empty list approves a Run whose changes GitHub could not list. None
    /// when the answer approved none.
    pub approved_control_files: Option<Vec<String>>,
    /// For a person's answer the factory read from their words (kept as its
    /// note), how sure it was, in percent; none for an answer given with a
    /// button, and for any answer not a person's.
    pub reply_confidence: Option<u8>,
    /// Whether the answer is the stand-in's, which the owner let stand
    /// ([`Gate::stand_in`] says why it gave it).
    pub by_stand_in: bool,
    /// The agent session that gave the answer for the owner, when the
    /// Orchestrator did rather than the owner in person; it is then not
    /// counted among the owner's own answers.
    pub agent_session_id: Option<Uuid>,
    /// When they answered.
    pub answered_at: DateTime<Utc>,
}

/// A reply the factory could not read as one of a Gate's answers, so it
/// asked the person back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AskedBack {
    /// The person's words.
    pub reply: String,
    /// When the factory asked back.
    pub at: DateTime<Utc>,
}

/// The stand-in's answer to a Gate, for the Run's owner: the answer it
/// gives, why, and until when the owner may override it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StandInAnswer {
    /// Its answer, one the Gate offers.
    pub answer: Outcome,
    /// Why, in a sentence or two.
    pub reason: String,
    /// How sure it was, in percent.
    pub confidence: u8,
    /// Until when the owner may override it; then the Run goes on with it.
    pub until: DateTime<Utc>,
}

/// Who asks a Gate's question, which says how it is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    /// The Flow asks it once its entry's Step ends `done`: a person picks
    /// one of its answers, which the Flow routes.
    #[default]
    Flow,
    /// An agent asked it, ending its Step `question`: the Run's owner
    /// writes the answer (the note of its one answer, `answer`), and the
    /// Step's Role goes on with it.
    Question,
}

impl GateKind {
    /// The kind of the Gate of a Step that ended with `outcome`: only an
    /// agent's question ends a Step `question`.
    pub fn of(outcome: Option<&Outcome>) -> Self {
        if outcome.is_some_and(|outcome| *outcome == Outcome::question()) {
            Self::Question
        } else {
            Self::Flow
        }
    }

    /// Whether anything but a person may answer it. An agent's question is
    /// never answered by anything but the Run's owner: whatever answers
    /// Gates on a person's behalf must leave question Gates alone.
    pub fn answered_only_by_a_person(self) -> bool {
        matches!(self, Self::Question)
    }
}

/// A question a Step's end puts to a person. The Run waits for the answer.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Gate {
    /// Who asks it: the Flow, or the Step's agent.
    pub kind: GateKind,
    /// The question.
    pub question: String,
    /// The answers it offers, in the Flow's order.
    pub answers: Vec<Outcome>,
    /// When the Run started waiting.
    pub opened_at: DateTime<Utc>,
    /// The person's answer; none while the Run waits.
    pub answer: Option<GateAnswer>,
    /// The latest reply the factory could not read as one of its answers,
    /// and asked back for; none when it never asked back.
    pub asked_back: Option<AskedBack>,
    /// The stand-in's answer for the Run's owner, while the owner may
    /// override it and after; none when the stand-in did not answer.
    pub stand_in: Option<StandInAnswer>,
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
    /// The rounds of the repository's checks it ran, in order: none for a
    /// Step that runs no checks.
    pub checks: Vec<CheckRound>,
}

/// The most rounds of checks a Step runs.
pub const MAX_CHECK_ROUNDS: u8 = 10;

/// How a round of a Step's checks ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckRoundOutcome {
    /// Every check passed.
    Passed,
    /// A check failed, or could not run.
    Failed,
}

impl CheckRoundOutcome {
    /// The word the outcome is kept and handed back as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

/// A round of checks that has ended.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckRoundEnd {
    /// How the checks went.
    pub outcome: CheckRoundOutcome,
    /// What each check did, for a person and for the agent's fix to read:
    /// the agent's report, or, for a verified round, the checks that failed
    /// with the end of their output.
    pub report: String,
    /// When the round ended.
    pub ended_at: DateTime<Utc>,
}

/// How one check of a verified round ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    /// The check's name.
    pub name: String,
    /// Its exit code: 0 when it passed.
    pub exit_code: i32,
    /// The end of what it printed, both streams, at most 4 KiB.
    pub output: String,
}

/// The most of a check's output a verified round keeps, once escaped as
/// JSON.
pub const MAX_CHECK_RESULT_OUTPUT_BYTES: usize = 4096;

/// One round of the repository's checks a Step ran on its agent's work, in
/// the agent's session.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckRound {
    /// Which round, from 1.
    pub round: u8,
    /// The names of the checks it runs, in the repository's order.
    pub checks: Vec<String>,
    /// When it started.
    pub started_at: DateTime<Utc>,
    /// How and when it ended; none while the checks run.
    pub end: Option<CheckRoundEnd>,
    /// Whether the engine runs the checks itself, on the pushed commit in a
    /// worktree of its own, reading each one's exit code; otherwise the
    /// agent runs them and reports how they went.
    pub verified: bool,
    /// How each check of a verified round ran so far, in the round's
    /// order; none for a round the agent reports.
    pub results: Vec<CheckResult>,
    /// The commit a verified round's checks run on: the head of the Run's
    /// branch on GitHub when its first check ran.
    pub checked_commit: Option<String>,
    /// What the agent handed back for the work the round checks.
    #[serde(skip)]
    pub work_output: String,
    /// The command of each check, at its name's place in `checks`.
    #[serde(skip)]
    pub commands: Vec<String>,
    /// The repository's checks file as the Step's first round read it,
    /// which every round goes by.
    #[serde(skip)]
    pub declared: String,
    /// The prompt that asks the agent to run the checks.
    #[serde(skip)]
    pub check_action_id: Uuid,
    /// The prompt that sent the round's failure back to the agent, once it
    /// was sent.
    #[serde(skip)]
    pub fix_action_id: Option<Uuid>,
    /// Whether the round failed because the agent's quota ran out, during
    /// its checks or the fix of their failure: no fix or round follows it.
    #[serde(skip)]
    pub quota_ran_out: bool,
}

/// A round of checks to record, before the agent is asked to run them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRoundStart {
    /// Which round, from 1 to [`MAX_CHECK_ROUNDS`].
    pub round: u8,
    /// What the agent handed back for the work the round checks.
    pub work_output: String,
    /// The names of the checks it runs; at least one.
    pub checks: Vec<String>,
    /// The command of each check, at its name's place in `checks`.
    pub commands: Vec<String>,
    /// The repository's checks file as the Step's first round read it.
    pub declared: String,
    /// The prompt, a UUIDv7, that asks the agent to run them.
    pub check_action_id: Uuid,
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
    /// The Agent the session is of; none for a session claimed before
    /// Steps recorded their Agent, which is of the first Agent of its Role.
    pub agent: Option<AgentName>,
    /// The bot whose harness runs the session, so it can be stopped and
    /// deleted whatever the agent definition says now; none for a session
    /// claimed before Steps recorded it.
    pub bot: Option<Uuid>,
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
    /// The session, with the action of its latest prompt: the first, or the
    /// one [`crate::domain::ports::RunRepository::begin_turn`] recorded.
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

/// What the agent of a Step handed back for one prompt of its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandedBack {
    /// The agent handed this back for the prompt.
    Handed(HandBack),
    /// Nothing yet: the prompt is the Step's latest, and the agent handed
    /// nothing back for it.
    Nothing,
    /// The Step went on to a later prompt of its session, whose hand back
    /// is not the prompt's; or it works in another session now.
    Superseded,
}

/// What an agent handed back for its Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandBack {
    /// One of the outcomes the Step offered.
    pub outcome: Outcome,
    /// What the agent produced, for a person and the Run's later Steps.
    pub output: String,
}
