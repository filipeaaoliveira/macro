//! What the factory's use cases take and return.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::{MainHold, RequiredChecksState};
use factory_records::domain::models::{
    EntryId, Outcome, Repository, Role, Run, RunId, RunOutcome, Step, StepKey,
};
use factory_records::domain::review_rounds::ReviewRound;
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::control_files::ControlFilesError;
use crate::domain::flow::{Entry, Flow, FlowError, StepName};

/// What a Run's workflow starts with. The Run's request stays in its
/// records, so a Run's history carries ids, not its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStart {
    /// The Run, already recorded.
    pub run_id: RunId,
    /// The Flow it walks.
    pub flow: Flow,
}

/// One Step for a worker to run. A field it does not know is refused, so a
/// worker never runs a Step it would misread: a worker older than a field
/// fails the activity, and a newer one picks it up. Fields are therefore
/// only ever added, with a default, never removed or renamed: Runs in
/// flight carry them in their histories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepRequest {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The entry it visits.
    pub entry: EntryId,
    /// The work it does.
    pub step: StepName,
    /// The Role an agent fills to do it, as its entry names.
    #[serde(default)]
    pub role: Option<Role>,
    /// Which visit of the entry it is, from 1.
    pub visit: u16,
    /// The review round the Step reviews in, by the place of the round's
    /// first Step; none for a Step of no review panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub round: Option<u16>,
    /// Whether its agent may ask the Run's owner a question, ending the
    /// Step `question`: its Flow lets agents ask.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub may_ask: bool,
    /// How many questions its work may still ask before the next one ends
    /// the Run for a person: the Flow's, less those the Steps it goes on
    /// from asked.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub questions_left: u8,
    /// The Step whose agent asked the question this Step goes on from, by
    /// its place: this Step works in that Step's session, with the owner's
    /// answer. None for a Step that starts its work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumes: Option<u16>,
}

impl StepRequest {
    /// The `visit` of `entry`, as the Step `key`: the work its entry names,
    /// and the Role it names for an agent to fill.
    pub fn of(key: StepKey, entry: &Entry, visit: u16) -> Self {
        Self {
            key,
            entry: entry.id.clone(),
            step: entry.step.clone(),
            role: entry.role.clone(),
            visit,
            round: None,
            may_ask: false,
            questions_left: 0,
            resumes: None,
        }
    }

    /// The Step `key` of the review round that starts at `round`, the
    /// `visit` of the review panel `entry`, in which a reviewer of `role`
    /// reviews.
    pub fn reviewer(key: StepKey, entry: &Entry, visit: u16, role: &Role, round: u16) -> Self {
        Self {
            key,
            entry: entry.id.clone(),
            step: entry.step.clone(),
            role: Some(role.clone()),
            visit,
            round: Some(round),
            may_ask: false,
            questions_left: 0,
            resumes: None,
        }
    }

    /// The Step `key` that goes on from this one's question, as the same
    /// visit of its entry, in its session.
    pub fn going_on(&self, key: StepKey) -> Self {
        Self {
            key,
            resumes: Some(self.key.seq),
            ..self.clone()
        }
    }
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// How a Step ended, for its workflow to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepEnding {
    /// The Step.
    pub key: StepKey,
    /// How it ended.
    pub report: StepReport,
}

/// What a Step's work is given: the Run's request, and what the Run's Steps
/// before it produced.
#[derive(Debug, Clone, Copy)]
pub struct StepInput<'a> {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Role an agent fills to do the Step, as its entry names.
    pub role: Option<&'a Role>,
    /// The Run's owner, as whom an agent does the Step.
    pub owner: &'a MacroUserIdStr<'static>,
    /// What the Run was asked to do.
    pub request: &'a str,
    /// Whether the Run's owner created it in their own signed-in session:
    /// only then does its request ask for the Control Files it names.
    pub created_in_person: bool,
    /// The Control File patterns the Run keeps from its creation; none for
    /// a Run created before Runs kept them, which the default checks.
    pub control_file_patterns: Option<&'a [String]>,
    /// The Macro task the Run works on: its task document's id.
    pub task_id: Option<Uuid>,
    /// The repository the Run works on, if it works on one.
    pub repository: Option<&'a Repository>,
    /// The Run's Steps that ended before this one, in order.
    pub earlier: &'a [Step],
    /// For a reviewer's Step, the review round it reviews in; for any
    /// other, the Run's latest review round that ended before it.
    pub review: Option<&'a ReviewRound>,
    /// Whether it is a reviewer's Step of a review panel.
    pub reviewer: bool,
    /// Whether its agent may ask the Run's owner a question.
    pub may_ask: bool,
    /// How many questions its work may still ask before the next one ends
    /// the Run for a person.
    pub questions_left: u8,
    /// The Step whose question it goes on from, by its place: its agent
    /// goes on in that Step's session, with the owner's answer.
    pub resumes: Option<u16>,
}

/// What an agent is asked to do for one Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTask {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Run's owner, as whom the agent works.
    pub owner: MacroUserIdStr<'static>,
    /// The Step.
    pub step: StepName,
    /// The Role the agent fills.
    pub role: Role,
    /// The Step's instructions, the Run's request and its earlier Steps.
    pub prompt: String,
    /// The outcomes the agent may end the Step with.
    pub outcomes: Vec<Outcome>,
    /// The repository the agent works in, as its
    /// `https://github.com/owner/name` URL, if the Run works on one.
    pub repo_url: Option<String>,
    /// When to stop waiting for the agent's answer, at the latest, when the
    /// Step sets a time of its own; the attempt's own deadline holds too.
    pub answer_by: Option<DateTime<Utc>>,
    /// The Step whose question this one goes on from, by its place: the
    /// agent is prompted in that Step's session, which it goes on in, not
    /// a new one. None for a Step that starts its work.
    pub resumes: Option<u16>,
    /// For a Step that goes on from a question, its whole prompt, the
    /// owner's answers in it: what a new session is prompted with when the
    /// asking Step's cannot be resumed.
    pub afresh: Option<String>,
}

/// A later prompt an agent is asked to work on, in the session it did its
/// Step's work in: a round of checks, or the fix of one that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurn {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Run's owner, as whom the agent works.
    pub owner: MacroUserIdStr<'static>,
    /// The Role the agent fills, whose Agent's session the Step works in.
    pub role: Role,
    /// The prompt's id in the session, a UUIDv7 the Step recorded before
    /// asking, so a retried Step asks under the same one.
    pub action_id: Uuid,
    /// Its place among the Step's later prompts: a later prompt has a
    /// higher one, whatever its id, so a stale attempt never takes the Step
    /// back to an earlier prompt.
    pub place: u16,
    /// What the agent is asked.
    pub prompt: String,
    /// The outcomes the agent may end the prompt with.
    pub outcomes: Vec<Outcome>,
    /// When to stop waiting for the agent's answer, at the latest: the
    /// Step's own time for its rounds.
    pub answer_by: DateTime<Utc>,
}

/// A command run in the checkout of the session a Step's agent works in,
/// beside the agent: one check of a round the engine runs itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The Run's owner, whose session it is.
    pub owner: MacroUserIdStr<'static>,
    /// The Role the agent fills, whose Agent's session the Step works in.
    pub role: Role,
    /// What to run, from the checkout's root.
    pub command: agent_session::domain::session_command::SessionCommand,
}

/// A GitHub repository, named by its owner and name: the two segments of
/// the `https://github.com/owner/name` URL a coding session takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeRepository {
    owner: String,
    name: String,
}

impl CodeRepository {
    /// The repository `url` names. It must be exactly
    /// `https://github.com/owner/name`, each segment of `[A-Za-z0-9._-]` and
    /// neither `.` nor `..`, with no `.git` ending or trailing slash: the
    /// canonical form a coding session's repository takes, so a Run and its
    /// sessions agree on which repository they work on. The segments are put
    /// into GitHub's API paths.
    pub fn parse(url: &str) -> Option<Self> {
        let path = url.strip_prefix("https://github.com/")?;
        let (owner, name) = path.split_once('/')?;
        let segment = |part: &str| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        };
        // A `.git` ending names the same repository as one without; one
        // form keeps a Run's record and its session's equal.
        if !segment(owner) || !segment(name) || name.ends_with(".git") {
            return None;
        }
        Some(Self {
            owner: owner.to_string(),
            name: name.to_string(),
        })
    }

    /// The account the repository lives under.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The repository's name, without its owner.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Its `https://github.com/owner/name` URL.
    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

/// The branch a Run's work is pushed to and proposed from. A Run has one, so
/// every Step of it, and every retry of a Step, finds the same branch and the
/// same pull request.
pub fn run_branch(run_id: RunId) -> String {
    format!("factory/{run_id}")
}

/// What a Run's owner may do with a repository on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryAccess {
    /// The branch it checks out by default.
    pub default_branch: String,
    /// Whether the owner may push to it.
    pub can_push: bool,
}

/// A pull request as the Run's owner sees it on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// Its number in its repository.
    pub number: u64,
    /// Its page on GitHub.
    pub url: String,
    /// Whether it is open, closed, or merged.
    pub state: PullRequestState,
    /// The commit its head branch is at.
    pub head_sha: String,
}

/// Whether a pull request is open, closed, or merged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestState {
    /// Open.
    Open,
    /// Closed without merging.
    Closed,
    /// Merged.
    Merged,
}

/// What GitHub says of the checks on a pull request's head commit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadChecks {
    /// Its check suites, one per app that checks it, such as a CI.
    pub suites: Vec<CheckSuite>,
    /// Its commit statuses, the latest of each context.
    pub statuses: Vec<CommitStatus>,
    /// Its check runs: every one when they were read to find required
    /// checks by name, else only those that failed, read to name them.
    pub runs: Vec<Check>,
    /// Whether a list was longer than was read, so these are not all its
    /// checks.
    pub truncated: bool,
    /// The apps, by slug, that hold CI on the head: a run not finished, or
    /// runs not all read, as a GitHub Actions run a concurrency group holds,
    /// whose suite may be empty or not made yet.
    pub held_apps: Vec<String>,
    /// Whether the owner's token may not read GitHub Actions runs, so a run
    /// that waits shows only through an empty Actions suite.
    pub actions_unread: bool,
}

/// The checks a pull request's base requires, as far as they could be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredChecks {
    /// The checks the readable sources name.
    pub checks: Vec<RequiredCheck>,
    /// Whether every source was read in full; a partial set may leave
    /// required checks out.
    pub complete: bool,
}

/// A check a pull request's base requires before it merges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredCheck {
    /// The check run's name, or the commit status's context.
    pub name: String,
    /// The app whose check run it must be; none when any app's run, or a
    /// commit status, will do.
    pub app_id: Option<u64>,
}

/// Where one check stands. GitHub's states map onto it in one place
/// ([`CheckState::of_check`], [`CheckState::of_status`]), so every reader
/// agrees on what passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckState {
    /// Not finished, or not started.
    Waiting,
    /// Finished, and lets a pull request merge.
    Passed,
    /// Finished, and does not: GitHub's conclusion or state, for people.
    Failed(String),
}

impl CheckState {
    /// A check suite's or run's state from GitHub's `status` and
    /// `conclusion`: a completed one passed when it succeeded, was neutral,
    /// or was skipped.
    pub fn of_check(status: &str, conclusion: Option<&str>) -> Self {
        if status != "completed" {
            return Self::Waiting;
        }
        match conclusion {
            Some("success" | "neutral" | "skipped") => Self::Passed,
            Some(conclusion) => Self::Failed(conclusion.to_string()),
            None => Self::Failed("no conclusion".to_string()),
        }
    }

    /// A commit status's state from GitHub's: `success` passed, `pending`
    /// waits, and `failure` or `error` failed.
    pub fn of_status(state: &str) -> Self {
        match state {
            "success" => Self::Passed,
            "pending" => Self::Waiting,
            other => Self::Failed(other.to_string()),
        }
    }
}

/// One app's checks on a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSuite {
    /// GitHub's id for it.
    pub id: u64,
    /// The app that runs it.
    pub app: String,
    /// Where it stands.
    pub state: CheckState,
    /// How many check runs it holds: an app that runs nothing on the commit
    /// leaves an empty suite queued for good, though a CI may add its runs
    /// a while after it made the suite.
    pub check_runs: u64,
    /// When GitHub created it.
    pub created_at: Option<DateTime<Utc>>,
}

/// The latest commit status of one context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitStatus {
    /// The context, such as a CI's name.
    pub context: String,
    /// Where it stands.
    pub state: CheckState,
    /// Its page, when it gives one.
    pub url: Option<String>,
}

/// One check run, such as a CI job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Its name, which a branch's required checks name it by.
    pub name: String,
    /// Where it stands.
    pub state: CheckState,
    /// Its page, when GitHub gives one.
    pub url: Option<String>,
    /// GitHub's id of the app that runs it.
    pub app_id: Option<u64>,
}

/// A pull request to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPullRequest {
    /// The branch whose commits it proposes.
    pub branch: String,
    /// The branch it proposes them to.
    pub base: String,
    /// Its title.
    pub title: String,
    /// Its description.
    pub body: String,
}

/// What asking GitHub to open a pull request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opening {
    /// GitHub opened it.
    Opened,
    /// GitHub declined, for this reason: one is already open for the
    /// branch, or the branch has nothing its base lacks.
    Refused(String),
}

/// What asking GitHub to merge a pull request came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merging {
    /// GitHub merged it.
    Merged,
    /// Its head is no longer the commit the merge named.
    HeadChanged,
    /// GitHub declined, for this reason.
    Refused(String),
}

/// What an agent handed back for a Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAnswer {
    /// One of the task's outcomes.
    pub outcome: Outcome,
    /// What it did, for a person and the Run's later Steps to read.
    pub text: String,
}

/// Whether a Step started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStart {
    /// The Step is recorded, and its work may begin.
    Started,
    /// The Step's Run ended before it started, as one cancelled, or ended
    /// for good by the sweep, before its workflow did. The workflow ends as
    /// the Run did.
    RunEnded(RunOutcome),
}

/// How a Step ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepReport {
    /// Its outcome, which the Flow routes.
    pub outcome: Outcome,
    /// What it produced, for a person to read.
    pub output: String,
}

/// A Run's end, for its workflow to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFinish {
    /// The Run.
    pub run_id: RunId,
    /// How it ended.
    pub outcome: RunOutcome,
}

/// A Run and the Steps it took.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    /// The Run.
    pub run: Run,
    /// Its Steps, in the order they started.
    pub steps: Vec<Step>,
    /// The Main Hold on the base branch the Run merges to, while the Run
    /// is at its merge and the base's required checks fail: the Run does
    /// not merge until it clears, even once its merge Gate is approved.
    pub main_hold: Option<MainHold>,
    /// Why the checks the base branch requires decide nothing, while the
    /// Run is at its merge: they could not be read, or the base requires
    /// none. Nothing holds the merge on them.
    pub base_required_checks: Option<RequiredChecksState>,
    /// Its review rounds, in order, each with what its reviewers found.
    pub review_rounds: Vec<ReviewRound>,
}

/// The Control File patterns a person's Runs are checked against: their
/// team's, or the factory's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ControlFilesSettings {
    /// The glob patterns, matched against each changed path from the
    /// repository's root, ignoring case.
    pub patterns: Vec<String>,
    /// Whether they are the factory's default: the team set none, or the
    /// person is on no team.
    pub is_default: bool,
    /// The team whose patterns they are; none when the person is on no
    /// team.
    pub team_id: Option<Uuid>,
    /// Whether the person may change them: they administer the team.
    pub can_edit: bool,
}

/// A Flow a person can start a Run of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowSummary {
    /// Its name, which a Run of it is started with.
    pub name: String,
    /// What it does, for a person.
    pub description: String,
    /// Whether a Run of it works on a GitHub repository, which starting it
    /// must name.
    pub repository: bool,
}

/// Errors of the factory's Run use cases.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// The Flow definition does not hold together.
    #[error(transparent)]
    InvalidFlow(#[from] FlowError),
    /// The request is empty.
    #[error("the request must not be empty")]
    EmptyRequest,
    /// The request is larger than a Run keeps.
    #[error("the request must be at most {max} bytes")]
    RequestTooLarge {
        /// The most bytes a request may hold.
        max: usize,
    },
    /// No Run has this id.
    #[error("run not found")]
    NotFound,
    /// No task the Run's owner can see has this id.
    #[error("no task the run's owner can see has this id")]
    UnknownTask,
    /// No task the caller can see has the id the path names.
    #[error("task not found")]
    TaskNotFound,
    /// No built-in Flow has this name.
    #[error("no flow has this name")]
    UnknownFlow,
    /// The Flow works on a repository, and the Run names none.
    #[error("this flow works on a repository: give its repoUrl")]
    RepositoryRequired,
    /// The Flow works on no repository, and the Run names one.
    #[error("this flow works on no repository")]
    RepositoryNotUsed,
    /// The repository is not named by a `https://github.com/owner/name` URL.
    #[error("the repository must be named by a https://github.com/owner/name URL")]
    InvalidRepository,
    /// The Run's owner cannot reach the repository with their GitHub
    /// account, has not connected one, or the repository has no branch yet.
    #[error(
        "the run's owner must connect GitHub to Macro and be able to reach the repository, which needs a branch"
    )]
    UnreachableRepository,
    /// The Run's owner may read the repository, but not push to it.
    #[error("the run's owner must be able to push to the repository")]
    NoPushAccess,
    /// The Gate does not offer the answer.
    #[error("the gate offers {offered:?}")]
    AnswerNotOffered {
        /// The answers it offers.
        offered: Vec<String>,
    },
    /// The Run is Tampered, and the answer approving its Gate does not
    /// approve exactly the Control Files its branch changed unasked.
    #[error(
        "the run's branch changes control files its request does not ask for: approve them by listing them in approvedControlFiles"
    )]
    ControlFilesNotApproved,
    /// Someone who does not administer a team set its Control Files, or is
    /// on no team.
    #[error("only an admin of a team can set its control files")]
    NotATeamAdmin,
    /// A list of Control File patterns does not hold together.
    #[error(transparent)]
    InvalidControlFiles(#[from] ControlFilesError),
    /// Temporal could not be reached or did not answer.
    #[error(transparent)]
    Upstream(#[from] upstream_error::UpstreamError),
    /// The document is not a task.
    #[error("the document is not a task")]
    NotATask,
    /// The user id names no account.
    #[error("no user has this id")]
    UnknownUser,
    /// A hand back came from outside a Step's agent session.
    #[error("only a factory step's agent session can hand back, from its code")]
    NotAStepSession,
    /// Someone other than the person in their own signed-in session
    /// answered a Gate or set their Control File patterns: an internal
    /// caller, an API key or token, or an agent's program acting for them.
    #[error(
        "only the person, signed in to Macro, can answer a run's gate or set its control files"
    )]
    NotAPerson,
    /// A hand back came from someone other than the Run's owner.
    #[error("only the run's owner can hand back its step")]
    NotTheOwner,
    /// A hand back named an outcome its Step does not offer.
    #[error("the step ends with one of {offered:?}")]
    OutcomeNotOffered {
        /// The outcomes the Step offers.
        offered: Vec<String>,
    },
    /// A question is empty, or larger than a Gate's question keeps.
    #[error("a question must not be empty, and must be at most {max} bytes")]
    QuestionSize {
        /// The most bytes a question may hold.
        max: usize,
    },
    /// An answer to an agent's question carries no words in its note.
    #[error("an agent's question is answered in the note: it must not be empty")]
    AnswerMissing,
    /// A hand back is larger than a Step's output keeps.
    #[error("what is handed back must be at most {max} bytes")]
    HandBackTooLarge {
        /// The most bytes it may hold.
        max: usize,
    },
    /// The Step already ended.
    #[error("the step has ended")]
    StepEnded,
    /// The Run has ended, so it takes no answer and cannot be cancelled
    /// again.
    #[error("the run has ended")]
    RunEnded,
    /// The note on an answer is larger than a Gate keeps.
    #[error("the note must be at most {max} bytes")]
    NoteTooLarge {
        /// The most bytes a note may hold.
        max: usize,
    },
    /// The Run's workflow did not take the answer, and said why: it waits
    /// on no Gate of that Step, or the Gate was answered.
    #[error("the answer was refused: {0}")]
    AnswerRefused(String),
    /// Any other internal error.
    #[error("internal factory error: {0:?}")]
    Internal(rootcause::Report),
}

impl From<rootcause::Report> for FactoryError {
    fn from(report: rootcause::Report) -> Self {
        FactoryError::Internal(report)
    }
}

/// Errors of running a Step or recording a Run's end.
#[derive(Debug, thiserror::Error)]
pub enum StepError {
    /// The Step's Run is not recorded.
    #[error("the step's run does not exist")]
    NoSuchRun,
    /// The Step's Run ended: before the Step started, as a Run cancelled
    /// before its workflow did, or while it worked.
    #[error("the step's run has ended")]
    RunEnded,
    /// The Step produced more output than a Step keeps.
    #[error("the step's output must be at most {max} bytes")]
    OutputTooLarge {
        /// The most bytes an output may hold.
        max: usize,
    },
    /// The Step's work failed in a way trying again would repeat.
    #[error("the step's work failed for good: {0:?}")]
    Final(rootcause::Report),
    /// Any other internal error.
    #[error("internal step error: {0:?}")]
    Internal(rootcause::Report),
}

impl From<rootcause::Report> for StepError {
    fn from(report: rootcause::Report) -> Self {
        StepError::Internal(report)
    }
}
