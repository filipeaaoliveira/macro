//! What the factory's use cases take and return.

use factory_records::domain::models::{
    EntryId, Outcome, Repository, Run, RunId, RunOutcome, Step, StepKey,
};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::flow::{Flow, FlowError, StepName};

/// What a Run's workflow starts with. The Run's request stays in its
/// records, so a Run's history carries ids, not its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStart {
    /// The Run, already recorded.
    pub run_id: RunId,
    /// The Flow it walks.
    pub flow: Flow,
}

/// One Step for a worker to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRequest {
    /// The Step: its Run and its place in it.
    pub key: StepKey,
    /// The entry it visits.
    pub entry: EntryId,
    /// The work it does.
    pub step: StepName,
    /// Which visit of the entry it is, from 1.
    pub visit: u16,
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
    /// The Run's owner, as whom an agent does the Step.
    pub owner: &'a MacroUserIdStr<'static>,
    /// What the Run was asked to do.
    pub request: &'a str,
    /// The Macro task the Run works on: its task document's id.
    pub task_id: Option<Uuid>,
    /// The repository the Run works on, if it works on one.
    pub repository: Option<&'a Repository>,
    /// The Run's Steps that ended before this one, in order.
    pub earlier: &'a [Step],
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
    /// The Step's instructions, the Run's request and its earlier Steps.
    pub prompt: String,
    /// The outcomes the agent may end the Step with.
    pub outcomes: Vec<Outcome>,
    /// The repository the agent works in, as its
    /// `https://github.com/owner/name` URL, if the Run works on one.
    pub repo_url: Option<String>,
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
    /// neither `.` nor `..`, as the agent egress reads a session's repository:
    /// the two ends must agree on which repository a Run works on, and the
    /// segments are put into GitHub's API paths.
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
        if !segment(owner) || !segment(name) {
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

/// A pull request as the Run's owner sees it on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// Its number in its repository.
    pub number: u64,
    /// Its page on GitHub.
    pub url: String,
    /// Whether it is open, closed, or merged.
    pub state: PullRequestState,
    /// The commit its head branch is at, when GitHub says.
    pub head_sha: Option<String>,
    /// The checks on that commit; none when they could not be read.
    pub checks: Option<Vec<Check>>,
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

/// One check on a commit, such as a CI job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Its name.
    pub name: String,
    /// GitHub's status: `queued`, `in_progress` or `completed`, among others.
    pub status: String,
    /// How it concluded, once completed: `success`, `failure` and so on.
    pub conclusion: Option<String>,
    /// Its page, when GitHub gives one.
    pub url: Option<String>,
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
}

/// A Flow a person can start a Run of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlowSummary {
    /// Its name, which a Run of it is started with.
    pub name: String,
    /// What it does, for a person.
    pub description: String,
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
    /// The Gate does not offer the answer.
    #[error("the gate offers {offered:?}")]
    AnswerNotOffered {
        /// The answers it offers.
        offered: Vec<String>,
    },
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
    /// answered a Gate: an internal caller, an API key or token, or an
    /// agent's program acting for them.
    #[error("only the run's owner, signed in to Macro, can answer its gate")]
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
