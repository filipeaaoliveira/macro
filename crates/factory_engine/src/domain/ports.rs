//! Ports (trait contracts) of the factory's use cases.

mod gate_replies;

pub use gate_replies::{GateModel, GateReplyService, ModelPick, ModelQuestion};

use agent_session::domain::session_command::SessionCommandOutput;
use agent_session::domain::session_task::TaskDocumentId;
use factory_records::domain::automations::Automation;
use factory_records::domain::models::{HandBack, Outcome, Role, RunId, StepKey, StepSession};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::automations::{
    AutomationRequest, AutomationsSettings, CreatedAutomation, FiringResult, RoutineFiring,
};
use crate::domain::flow::{StepName, StepUse};
use crate::domain::gate::{
    AnswerGate, GateAnswered, GateOpening, GateRuling, GateTelling, ReplyGate,
};
use crate::domain::gate_owner::{GateNotice, OwnerNote};
use crate::domain::main_hold::{HoldTask, MainLook, MainWatch};
use crate::domain::models::{
    AgentAnswer, AgentCommand, AgentTask, AgentTurn, Answerer, CodeRepository,
    ControlFilesSettings, Deployment, FactoryError, FlowSummary, HeadChecks, Merging,
    OpenPullRequest, Opening, PullRequest, RepositoryAccess, RequiredChecks, RunDetail, RunFinish,
    RunStart, SessionSteer, SessionStepRef, StandingRulesSettings, StepEnding, StepError,
    StepInput, StepReport, StepRequest, StepStart,
};
use crate::domain::review_rounds::{ChangedFile, ReviewRoundClosing, ReviewRoundOpening};
use crate::domain::standing_rules::RuleAnswer;
use crate::domain::step_terminal::TerminalRequest;
use crate::domain::task_thread::ThreadComment;
use factory_records::domain::standing_rules::OrchestratorDefinition;

/// What a Run's workflow did with an answer it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerDelivery {
    /// The workflow took the answer; the Run goes on.
    Taken,
    /// The workflow refused the answer, for this reason.
    Refused(String),
}

/// Why a Run's workflow could not be reached.
#[derive(Debug, thiserror::Error)]
pub enum RunWorkflowsError {
    /// The workflow service was unavailable or answered with something
    /// unusable: worth trying again later.
    #[error(transparent)]
    Unavailable(#[from] upstream_error::UpstreamError),
    /// Anything else.
    #[error("the run's workflow failed: {0:?}")]
    Other(rootcause::Report),
}

/// Starts, answers and cancels the workflows that walk Runs.
pub trait RunWorkflows: Send + Sync + 'static {
    /// Start the workflow of `start.run_id`. Starting a Run that already
    /// started changes nothing.
    fn start(&self, start: &RunStart)
    -> impl Future<Output = Result<(), RunWorkflowsError>> + Send;

    /// Whether the workflow of `run_id` exists, running or not.
    fn exists(&self, run_id: RunId)
    -> impl Future<Output = Result<bool, RunWorkflowsError>> + Send;

    /// Send `answer` to the workflow of `run_id`, which takes it or says why
    /// not.
    fn answer(
        &self,
        run_id: RunId,
        answer: &AnswerGate,
    ) -> impl Future<Output = Result<AnswerDelivery, RunWorkflowsError>> + Send;

    /// Send a person's words, `reply`, to the workflow of `run_id`, which
    /// takes them to read or says why not.
    fn reply(
        &self,
        run_id: RunId,
        reply: &ReplyGate,
    ) -> impl Future<Output = Result<AnswerDelivery, RunWorkflowsError>> + Send;

    /// Ask the workflow of `run_id` to stop its Step or Gate and end the Run
    /// cancelled. Asking again changes nothing. Whether the workflow was
    /// still there to ask: false once it completed.
    fn cancel(&self, run_id: RunId)
    -> impl Future<Output = Result<bool, RunWorkflowsError>> + Send;
}

/// Whether failed work is worth trying again: false when it would fail the
/// same way, such as an agent's turn that ended without a result.
pub trait Retryable {
    /// Whether trying the work again may succeed.
    fn retryable(&self) -> bool;
}

/// How an agent's failure bears on a Step that goes on after the agent's
/// work, as one that runs the repository's checks does.
pub trait AgentFailure: Retryable {
    /// Whether the agent could not go on because its owner's quota for it
    /// ran out.
    fn out_of_quota(&self) -> bool {
        false
    }

    /// The agent's last message, when it stopped without handing back a
    /// result even once reminded: the Step then waits for its Run's owner.
    fn stopped_without_result(&self) -> Option<&str> {
        None
    }
}

/// Does the work of a Flow's Steps.
pub trait StepExecutor: Send + Sync + 'static {
    /// The error type returned when a Step's work fails.
    type Err: std::error::Error + AgentFailure + Send + Sync + 'static;

    /// How it does `step`'s work, if it does.
    fn serves(&self, step: &StepName) -> Option<StepUse>;

    /// Do `step`'s work on `input`. A Temporal retry may call it again for
    /// the same Step: it goes on with the work, never starts it twice.
    fn run(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> impl Future<Output = Result<StepReport, Self::Err>> + Send;

    /// Stop the work of the Step `key` for good, as its Run was cancelled or
    /// its work failed for good, and let go of what it held. Stopping work
    /// that never started, or that ended, changes nothing.
    fn stop(
        &self,
        step: &StepName,
        role: Option<&Role>,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Let go of what the work of the Step `key` held, once its end is
    /// recorded. No retry of its work comes after that.
    fn release(
        &self,
        step: &StepName,
        role: Option<&Role>,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Act on `session`, the one the running Step `key` works in, as the
    /// agent of `role` that works there, for its owner's Step Terminal. An
    /// error says the session did not take a prompt: the agent never reads
    /// it. Work no agent does holds no session, so nothing steers it.
    fn steer(
        &self,
        role: &Role,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
        session: &StepSession,
        steer: SessionSteer,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send {
        let _ = (role, key, owner, session, steer);
        async { Ok(()) }
    }
}

/// An agent that does a Step's work: it reads the task's prompt, works, and
/// ends with one of the task's outcomes.
pub trait StepAgent: Send + Sync + 'static {
    /// The error type returned when the agent cannot do the task.
    type Err: std::error::Error + AgentFailure + Send + Sync + 'static;

    /// Do `task` and hand back how it ended. A Temporal retry may ask again
    /// for the same Step: the agent goes on with it, never starts it twice.
    fn answer(
        &self,
        task: AgentTask,
    ) -> impl Future<Output = Result<AgentAnswer, Self::Err>> + Send;

    /// Work on `turn`, a later prompt of the Step's work, in the session
    /// the Step's [`Self::answer`] worked in, of the Agent of `turn`'s Role
    /// that took the Step, and hand back how it ended. A
    /// Temporal retry may ask again for the same prompt: the agent goes on
    /// with it, never starts it twice.
    fn follow_up(
        &self,
        turn: AgentTurn,
    ) -> impl Future<Output = Result<AgentAnswer, Self::Err>> + Send;

    /// Run `command` in the checkout of the session the Step works in,
    /// beside the agent, and hand back how it ended, whatever its exit
    /// code. None when the session's runtime runs no command (a paired
    /// machine, a cloud agent): only the agent can run it there. A
    /// Temporal retry may run it again.
    fn run_check(
        &self,
        command: AgentCommand,
    ) -> impl Future<Output = Result<Option<SessionCommandOutput>, Self::Err>> + Send;

    /// Stop working on the Step `key` of `role` for `owner`, for good.
    fn stop(
        &self,
        key: StepKey,
        role: &Role,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Let go of the Step `key`'s work of `role` for `owner`, which ended
    /// and whose end is recorded.
    fn release(
        &self,
        key: StepKey,
        role: &Role,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Act on `session`, the one the running Step `key` of `role` works in,
    /// for `owner`'s Step Terminal: send it a prompt, which the agent reads
    /// once its turn ends, or stop its turn. An error says the session did
    /// not take the prompt; a prompt whose delivery is unknown may still
    /// land, so it is taken. An agent with no sessions has nothing to steer.
    fn steer(
        &self,
        key: StepKey,
        role: &Role,
        owner: &MacroUserIdStr<'static>,
        session: &StepSession,
        steer: SessionSteer,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send {
        let _ = (key, role, owner, session, steer);
        async { Ok(()) }
    }
}

/// Pull requests on GitHub, read and changed as a Run's owner, with the
/// GitHub account they connected to Macro.
pub trait PullRequests: Send + Sync + 'static {
    /// The error type returned when GitHub cannot be asked.
    type Err: std::error::Error + Retryable + Send + Sync + 'static;

    /// What `owner` may do with `repository`: none when they cannot reach
    /// it, have connected no GitHub account, or it has no branch yet.
    fn repository_access(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
    ) -> impl Future<Output = Result<Option<RepositoryAccess>, Self::Err>> + Send;

    /// The newest pull request from `branch` of `repository`, open or not.
    fn find(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
    ) -> impl Future<Output = Result<Option<PullRequest>, Self::Err>> + Send;

    /// [`Self::find`], with the checks on the pull request's head, read
    /// with one token: what a CI wait reads on each look. All its check runs
    /// are read when `all_runs`, to find required checks by name; otherwise
    /// only those of a failed suite.
    fn find_with_checks(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
        all_runs: bool,
    ) -> impl Future<Output = Result<Option<(PullRequest, HeadChecks)>, Self::Err>> + Send;

    /// The commit at the head of `branch` of `repository`, with the checks
    /// on it, read with one token: what a watch of a base branch reads on
    /// each look. None when `owner` reaches no such branch. All its check
    /// runs are read when `all_runs`; otherwise only those of a failed
    /// suite.
    fn branch_checks(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
        all_runs: bool,
    ) -> impl Future<Output = Result<Option<(String, HeadChecks)>, Self::Err>> + Send;

    /// The checks a pull request to `base` must pass before it merges, from
    /// its branch protection and rulesets, as far as they could be read;
    /// none when `owner` cannot tell which checks they are.
    fn required_checks(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        base: &str,
    ) -> impl Future<Output = Result<Option<RequiredChecks>, Self::Err>> + Send;

    /// The text of the file at `path` on `branch` of `repository`; none
    /// when the branch holds no file there.
    fn file_text(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
        path: &str,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// The paths `branch` of `repository` changed against its merge base
    /// with `base`; none when GitHub cannot list them all.
    fn changed_paths(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        base: &str,
        branch: &str,
    ) -> impl Future<Output = Result<Option<Vec<String>>, Self::Err>> + Send;

    /// The commit `branch` of `repository` is at; none when the branch is
    /// not there.
    fn branch_head(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// The files `head` of `repository` changed against its merge base
    /// with `base`, with the lines each change touches; none when GitHub
    /// cannot list them all.
    fn changed_files(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        base: &str,
        head: &str,
    ) -> impl Future<Output = Result<Option<Vec<ChangedFile>>, Self::Err>> + Send;

    /// The newest `count` deployments of `repository` to `environment`,
    /// newest first: of the commit `sha` only, when given.
    fn deployments(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        sha: Option<&str>,
        environment: &str,
        count: u8,
    ) -> impl Future<Output = Result<Vec<Deployment>, Self::Err>> + Send;

    /// The first parent of the commit `sha` of `repository`: for a merge
    /// commit, the base branch as it was before the merge. None for a
    /// commit with no parent.
    fn first_parent(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        sha: &str,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// Whether the commit `head` of `repository` contains the commit
    /// `ancestor`: it is that commit, or comes after it.
    fn contains(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        ancestor: &str,
        head: &str,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Ask GitHub to open `pull_request` in `repository`.
    fn open(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        pull_request: &OpenPullRequest,
    ) -> impl Future<Output = Result<Opening, Self::Err>> + Send;

    /// Ask GitHub to merge pull request `number` of `repository` while its
    /// head is the commit `head_sha`.
    fn merge(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        number: u64,
        head_sha: &str,
    ) -> impl Future<Output = Result<Merging, Self::Err>> + Send;
}

/// Asks the watch of a base branch's checks, which sets and clears its
/// Main Hold, to look again.
pub trait MainWatches: Send + Sync + 'static {
    /// The error type returned when the watch cannot be asked.
    type Err: std::error::Error + Retryable + Send + Sync + 'static;

    /// Ask the watch of `watch`'s base branch to look again, as the owner
    /// of `watch`'s Run among others, starting it when none runs. A watch
    /// about to end looks again instead.
    fn watch(&self, watch: &MainWatch) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// Files the task that names the merges a red base branch may come from.
pub trait HoldTasks: Send + Sync + 'static {
    /// The error type returned when the task cannot be filed.
    type Err: std::error::Error + Send + Sync + 'static;

    /// File `task` as `owner`, in the initiative of the task document
    /// `beside` when it has one, under the id `task_id`, and return the task
    /// document's id. A task already filed under `task_id`, as by a filing
    /// whose answer was lost, is found, not filed again.
    fn file(
        &self,
        owner: &MacroUserIdStr<'static>,
        beside: Option<Uuid>,
        task: &HoldTask,
        task_id: Uuid,
    ) -> impl Future<Output = Result<Uuid, Self::Err>> + Send;

    /// Comment `text` on the passage `quote` of the task document
    /// `task_id`, as `owner`.
    fn comment(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        quote: &str,
        text: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// Tells a Run's owner that one of its Gates waits on them.
pub trait GateNotifier: Send + Sync + 'static {
    /// Send `notice`. Sending it again is the same notification. A notice
    /// that cannot be sent is logged and dropped: the Gate waits all the
    /// same, and is listed as waiting.
    fn notify(&self, notice: GateNotice) -> impl Future<Output = ()> + Send;

    /// Mark the notification `notification_id` of `owner` done: its Gate
    /// was answered. One never sent changes nothing; one that cannot be
    /// marked is logged and left.
    fn answered(
        &self,
        owner: &MacroUserIdStr<'static>,
        notification_id: Uuid,
    ) -> impl Future<Output = ()> + Send;
}

/// Tells a paused Step's owner, quietly, that its work's time runs out.
pub trait PauseNotices: Send + Sync + 'static {
    /// Tell the owner of the paused Step `key` that it ends failed at
    /// `ends_by` unless they resume it. Telling them again is the same
    /// notice; one that cannot be told is logged and dropped.
    fn running_out(
        &self,
        key: StepKey,
        ends_by: chrono::DateTime<chrono::Utc>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Reads what a Run's owner commented on the Run's task.
pub trait TaskNotes: Send + Sync + 'static {
    /// The error type returned when the comments cannot be read now.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The comments `owner` left on the task document `task_id` at or
    /// after `since`, oldest first, read as `owner`: none once they no
    /// longer see the task, or it is gone.
    fn owner_notes(
        &self,
        task_id: Uuid,
        owner: &MacroUserIdStr<'static>,
        since: chrono::DateTime<chrono::Utc>,
    ) -> impl Future<Output = Result<Vec<OwnerNote>, Self::Err>> + Send;
}

/// Whether a comment on a Run's task was posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPosted {
    /// It is posted, now or before.
    Posted,
    /// The task, or the thread it replies to, is not there for the Run's
    /// owner.
    NoThread,
}

/// Posts a Run's updates on its task, as its owner.
pub trait TaskThread: Send + Sync + 'static {
    /// The error type returned when a comment cannot be posted.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Post `comment` on the task document `task_id` as `owner`, notifying
    /// no one. Posting a comment again, by its id, posts nothing more.
    fn post(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        comment: &ThreadComment,
    ) -> impl Future<Output = Result<ThreadPosted, Self::Err>> + Send;

    /// The first comment of the thread on the task document `task_id` that
    /// the factory posted as `owner` and that names `marker`; none when
    /// there is none.
    fn find_thread(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        marker: &str,
    ) -> impl Future<Output = Result<Option<Uuid>, Self::Err>> + Send;

    /// The link that opens the Run `run_id` in Macro.
    fn run_link(&self, run_id: RunId) -> String;
}

/// The task thread where nothing serves it: no update is posted.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTaskThread;

impl TaskThread for NoTaskThread {
    type Err = std::convert::Infallible;

    async fn post(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _task_id: Uuid,
        _comment: &ThreadComment,
    ) -> Result<ThreadPosted, Self::Err> {
        Ok(ThreadPosted::Posted)
    }

    async fn find_thread(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _task_id: Uuid,
        _marker: &str,
    ) -> Result<Option<Uuid>, Self::Err> {
        Ok(None)
    }

    fn run_link(&self, run_id: RunId) -> String {
        format!("/app/factory/runs/{run_id}")
    }
}

/// The owner section's ports where nothing serves them: no Gate notifies
/// anyone, and no comment is read.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoOwnerGates;

impl GateNotifier for NoOwnerGates {
    async fn notify(&self, _notice: GateNotice) {}

    async fn answered(&self, _owner: &MacroUserIdStr<'static>, _notification_id: Uuid) {}
}

impl TaskNotes for NoOwnerGates {
    type Err = std::convert::Infallible;

    async fn owner_notes(
        &self,
        _task_id: Uuid,
        _owner: &MacroUserIdStr<'static>,
        _since: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<OwnerNote>, Self::Err> {
        Ok(Vec::new())
    }
}

/// Inbound service port: what the watch of a base branch asks its worker
/// to do.
pub trait MainWatchService: Send + Sync + 'static {
    /// Look at the checks on the head of `watch`'s base branch once: hold
    /// the base when its required checks fail, filing the task with the
    /// suspect merges once per red period, and clear the hold when they
    /// pass. GitHub that cannot be read leaves the base as it was.
    fn look(&self, watch: MainWatch) -> impl Future<Output = Result<MainLook, StepError>> + Send;
}

/// The people Macro knows.
pub trait UserDirectory: Send + Sync + 'static {
    /// The error type returned when the directory cannot be read.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Whether `user_id` names an account.
    fn user_exists(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;
}

/// What a person may know of a task document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAccess {
    /// A task the person can see.
    Visible,
    /// A document the person can see, but not a task.
    NotATask,
    /// No document, a deleted one, or one the person cannot see: the same
    /// answer, so a hidden task is not revealed.
    NotFound,
}

/// The tasks Macro keeps, as one person sees them.
pub trait TaskDirectory: Send + Sync + 'static {
    /// The error type returned when the check cannot be made.
    type Err: std::error::Error + Send + Sync + 'static;

    /// What `user_id` may know of the task document `task`.
    fn task_access(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task: &TaskDocumentId,
    ) -> impl Future<Output = Result<TaskAccess, Self::Err>> + Send;

    /// The Project (an initiative) the task document `task_id` is in, as
    /// its Project property names it and `user_id` sees it; none when it is
    /// in none, or `user_id` no longer sees it.
    fn task_project(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> impl Future<Output = Result<Option<Uuid>, Self::Err>> + Send;

    /// Whether `user_id` may edit the Project (an initiative) `project_id`:
    /// one they cannot reach, or that is gone, is a no.
    fn may_edit_project(
        &self,
        user_id: &MacroUserIdStr<'static>,
        project_id: Uuid,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Make `follow_up` a task of `user_id`'s, with the id `task_id`, in
    /// the project of the task document `of` when it has one, unless a
    /// task with that id is already there: then it was made before. An
    /// error is one worth trying again, as an outage.
    fn create_follow_up(
        &self,
        user_id: &MacroUserIdStr<'static>,
        of: Option<Uuid>,
        follow_up: &FollowUpTask,
        task_id: Uuid,
    ) -> impl Future<Output = Result<FollowUpMade, Self::Err>> + Send;

    /// Comment `text` on the passage `quote` of the follow-up task
    /// `task_id`, as `user_id`, its owner.
    fn comment_on_follow_up(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
        quote: &str,
        text: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// Whether a follow-up task is there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowUpMade {
    /// The task is there: made now, or by an earlier attempt.
    Made,
    /// Document storage refused it, as when the request does not read:
    /// asking again would be refused too.
    Refused,
    /// Document storage made it under another id: asking again would make
    /// another, so the round stops.
    OtherId(String),
}

/// A finding outside the diff a review round reviewed, as a task of its
/// own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpTask {
    /// The task's name.
    pub name: String,
    /// What it says, as markdown.
    pub markdown: String,
}

/// Inbound service port: the Run API used by drivers (HTTP). Every call but
/// a hand back is made for an acting user, and reaches only that user's Runs.
pub trait FactoryService: Send + Sync + 'static {
    /// The built-in Flows a Run can be started of: those that name only
    /// Steps the worker serves.
    fn list_flows(&self) -> Vec<FlowSummary>;

    /// Record `actor`'s new Run of the built-in Flow named `flow` for
    /// `request`, working on the task document `task_id` if one is given,
    /// and on the GitHub repository `repo_url` if the Flow works on one,
    /// and start it. The actor must have an account, see the task, and
    /// reach the repository with their GitHub account. `in_person` says
    /// whether they create it in their own signed-in session.
    fn create_run(
        &self,
        actor: MacroUserIdStr<'static>,
        flow: String,
        request: String,
        task_id: Option<Uuid>,
        repo_url: Option<String>,
        in_person: bool,
    ) -> impl Future<Output = Result<RunDetail, FactoryError>> + Send;

    /// `actor`'s Run and the Steps it took. Reading a Run whose workflow
    /// never started, once it is old enough that a started one would have
    /// taken a Step, ends it `failed`: the one read that writes.
    fn get_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> impl Future<Output = Result<RunDetail, FactoryError>> + Send;

    /// `actor`'s Runs on the task document `task_id`, newest first, each
    /// with its Steps: at most the newest [`TASK_RUNS_LISTED`]. The actor
    /// must see the task.
    ///
    /// [`TASK_RUNS_LISTED`]: crate::domain::service::TASK_RUNS_LISTED
    fn list_task_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> impl Future<Output = Result<Vec<RunDetail>, FactoryError>> + Send;

    /// `actor`'s Runs that wait on them at a Gate no one answered yet,
    /// newest first, each with its Steps: at most the newest
    /// [`WAITING_RUNS_LISTED`].
    ///
    /// [`WAITING_RUNS_LISTED`]: crate::domain::service::WAITING_RUNS_LISTED
    fn list_waiting_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<Vec<RunDetail>, FactoryError>> + Send;

    /// `actor`'s Runs that hold a place in their Queue before asking them
    /// anything: those of a Flow with a Gate of its own that have not
    /// ended or reached a Gate yet, newest first, each with its Steps: at
    /// most the newest [`WAITING_RUNS_LISTED`]. Cancelling one frees its
    /// place, as for a Run whose workflow start could not be told.
    ///
    /// [`WAITING_RUNS_LISTED`]: crate::domain::service::WAITING_RUNS_LISTED
    fn list_starting_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<Vec<RunDetail>, FactoryError>> + Send;

    /// Link `actor`'s Run to the task document `task_id`, replacing any task
    /// it had. The actor must see the task.
    fn link_task(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
        task_id: Uuid,
    ) -> impl Future<Output = Result<RunDetail, FactoryError>> + Send;

    /// The Step of one of `actor`'s Runs that works, or worked, in the agent
    /// session `session_id`: the one that has not ended, else the latest.
    /// Not found when no Step of theirs does.
    fn session_step(
        &self,
        actor: &MacroUserIdStr<'static>,
        session_id: Uuid,
    ) -> impl Future<Output = Result<SessionStepRef, FactoryError>> + Send;

    /// Steer the running Step `seq` of `actor`'s Run from its Step
    /// Terminal: send its agent a note, pause it, resume it, or stop it
    /// (`crate::domain::step_terminal`). The actor is the Run's owner, in
    /// their own signed-in session; an agent's account is refused.
    fn steer_step(
        &self,
        actor: MacroUserIdStr<'static>,
        id: RunId,
        seq: u16,
        request: TerminalRequest,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Give `actor`'s words, `reply`, to the Gate the actor's Run waits on,
    /// in place of one of its answers: the factory reads them as one of its
    /// answers or asks back, and an agent's question takes them as the
    /// answer.
    fn reply_to_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        id: RunId,
        seq: u16,
        reply: String,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Give `actor`'s answer to the Gate the actor's Run waits on. Approving
    /// a Gate of a Tampered Run needs `approved_control_files`: the
    /// Tampered files the actor saw and approves, which must be the Run's.
    /// `answerer` says who gives it: the actor in person, or an agent
    /// session answering for them, which may answer only what a Standing
    /// Rule could and never what the factory keeps for a person
    /// ([`PersonOnly`](crate::domain::models::PersonOnly)).
    #[expect(clippy::too_many_arguments, reason = "each is a part of one answer")]
    fn answer_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        id: RunId,
        seq: u16,
        answer: Outcome,
        note: Option<String>,
        approved_control_files: Option<Vec<String>>,
        answerer: Answerer,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// The Control File patterns `actor`'s Runs are checked against: their
    /// team's, or the factory's default.
    fn control_files(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<ControlFilesSettings, FactoryError>> + Send;

    /// Set the Control File patterns of `actor`'s team's Runs; none goes
    /// back to the factory's default. The actor must administer the team.
    fn set_control_files(
        &self,
        actor: MacroUserIdStr<'static>,
        patterns: Option<Vec<String>>,
    ) -> impl Future<Output = Result<ControlFilesSettings, FactoryError>> + Send;

    /// The Standing Rules `actor`'s Runs are created with: their team's
    /// Orchestrator Definition, or the factory's default, and the team's
    /// Projects' own.
    fn standing_rules(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<StandingRulesSettings, FactoryError>> + Send;

    /// Set the Orchestrator Definition of `actor`'s team; none goes back to
    /// the factory's default. The actor must administer the team.
    fn set_standing_rules(
        &self,
        actor: MacroUserIdStr<'static>,
        definition: Option<OrchestratorDefinition>,
    ) -> impl Future<Output = Result<StandingRulesSettings, FactoryError>> + Send;

    /// Set the Orchestrator Definition of `actor`'s team for the Project
    /// `project_id`; none removes it, so the team's applies. The actor must
    /// administer the team and may edit the Project.
    fn set_project_rules(
        &self,
        actor: MacroUserIdStr<'static>,
        project_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> impl Future<Output = Result<StandingRulesSettings, FactoryError>> + Send;

    /// Keep what the agent working in the session `session_id`, acting as
    /// `user_id`, hands back for its Step, replacing an earlier hand back.
    fn hand_back(
        &self,
        session_id: Uuid,
        user_id: MacroUserIdStr<'static>,
        hand_back: HandBack,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Clear the Main Hold on the branch `base_branch` of the repository
    /// `repo_url`, for an operator: a hold no look can clear, as when no
    /// owner reaches the repository any more. A later red look holds the
    /// base again. Whether a hold was set.
    fn clear_main_hold(
        &self,
        repo_url: &str,
        base_branch: &str,
    ) -> impl Future<Output = Result<bool, FactoryError>> + Send;

    /// Cancel `actor`'s Run. Cancelling a Run that ended cancelled changes
    /// nothing.
    fn cancel_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Check that `actor` could start a Run of the built-in Flow named
    /// `flow` on the repository `repo_url`, as [`Self::create_run`] checks:
    /// the Flow exists, takes a repository exactly when one is given, and
    /// the actor's GitHub account reaches it and may push to it.
    fn check_run_order(
        &self,
        actor: &MacroUserIdStr<'static>,
        flow: &str,
        repo_url: Option<&str>,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Record and start the Run `automation`'s firing keyed `firing_key`
    /// starts for `request`, owned by its creator, on the task `task_id` if one is given: as
    /// [`Self::create_run`] for a Run its creator did not start in person,
    /// keeping which Automation started it. `TaskBusy` when a Run another
    /// Automation started runs on the task, `FiringRepeated` when the
    /// firing started one already.
    fn create_automation_run(
        &self,
        automation: &Automation,
        firing_key: &str,
        request: String,
        task_id: Option<Uuid>,
    ) -> impl Future<Output = Result<RunDetail, FactoryError>> + Send;
}

/// The Macro routines that fire Automations: a schedule's, a repository
/// event's and a Task event's trigger is a routine of the Automation's
/// creator, whose code step fires it. Every call acts as the creator.
pub trait AutomationTriggers: Send + Sync + 'static {
    /// Make the routine that fires `automation`, enabled, and return its
    /// id.
    fn create(
        &self,
        automation: &Automation,
    ) -> impl Future<Output = Result<Uuid, AutomationTriggerError>> + Send;

    /// Switch the routine `routine_id` of `owner` on or off.
    fn set_enabled(
        &self,
        owner: &MacroUserIdStr<'static>,
        routine_id: Uuid,
        enabled: bool,
    ) -> impl Future<Output = Result<(), AutomationTriggerError>> + Send;

    /// Delete the routine `routine_id` of `owner`; one already gone is
    /// deleted.
    fn delete(
        &self,
        owner: &MacroUserIdStr<'static>,
        routine_id: Uuid,
    ) -> impl Future<Output = Result<(), AutomationTriggerError>> + Send;
}

/// Why a routine that fires an Automation could not be made or changed.
#[derive(Debug, thiserror::Error)]
pub enum AutomationTriggerError {
    /// The scheduler refused it, and said why: an invalid schedule, event
    /// routines its creator may not have, or no linked GitHub account.
    #[error("the routine was refused: {0}")]
    Refused(String),
    /// The scheduler could not be reached, or failed.
    #[error("the scheduler is unavailable: {0:?}")]
    Unavailable(rootcause::Report),
}

/// Inbound service port: a team's Automations and their firings.
pub trait AutomationService: Send + Sync + 'static {
    /// `actor`'s team's Automations.
    fn automations(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<AutomationsSettings, FactoryError>> + Send;

    /// Keep a new Automation of `actor`'s team, which acts as `actor`.
    /// Only the actor in person makes one.
    fn create_automation(
        &self,
        actor: MacroUserIdStr<'static>,
        in_person: bool,
        request: AutomationRequest,
    ) -> impl Future<Output = Result<CreatedAutomation, FactoryError>> + Send;

    /// Switch `actor`'s team's Automation `id` on or off. Its creator
    /// switches it on, in person (`in_person`) from an account that is not
    /// an agent's; its creator or an admin of the team switches it off.
    fn set_automation_enabled(
        &self,
        actor: &MacroUserIdStr<'static>,
        in_person: bool,
        id: Uuid,
        enabled: bool,
    ) -> impl Future<Output = Result<Automation, FactoryError>> + Send;

    /// Delete `actor`'s team's Automation `id`: its creator's or, for an
    /// admin of the team, any.
    fn delete_automation(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: Uuid,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Fire the Automation the routine `routine_id` fires, from one of its
    /// runs, which acts as `actor`.
    fn fire_from_routine(
        &self,
        actor: &MacroUserIdStr<'static>,
        routine_id: Uuid,
        firing: RoutineFiring,
    ) -> impl Future<Output = Result<FiringResult, FactoryError>> + Send;

    /// Fire the webhook Automation `id` from a call whose `body` is signed
    /// as `signature` at `timestamp` (Unix seconds).
    fn fire_webhook(
        &self,
        id: Uuid,
        timestamp: &str,
        signature: &str,
        body: &[u8],
    ) -> impl Future<Output = Result<FiringResult, FactoryError>> + Send;
}

/// Inbound service port: what a Run's workflow asks its worker to do for a
/// review panel, around its reviewers' Steps.
pub trait ReviewService: Send + Sync + 'static {
    /// Record the review round `opening` starts: the commit of the Run's
    /// branch it reviews, and where the diff under review starts. Whether
    /// it opened: a round that cannot, as when the Run's branch is gone, is
    /// recorded as failed, with why.
    fn open_round(
        &self,
        opening: ReviewRoundOpening,
    ) -> impl Future<Output = Result<bool, StepError>> + Send;

    /// End the review round `closing` names, once its reviewers' Steps
    /// ended: tell its findings in the diff under review, its Blockers, from
    /// those outside it, which become follow-up tasks, and return the
    /// round's outcome for the Flow to route.
    fn close_round(
        &self,
        closing: ReviewRoundClosing,
    ) -> impl Future<Output = Result<Outcome, StepError>> + Send;
}

/// Inbound service port: what a Run's workflow asks its worker to do. Each
/// record is its own call, so a failed write never repeats a Step's work.
pub trait StepService: Send + Sync + 'static {
    /// Record that a Step started, unless its Run already ended.
    fn start_step(
        &self,
        request: StepRequest,
    ) -> impl Future<Output = Result<StepStart, StepError>> + Send;

    /// Do a Step's work, and say how it ended. It records nothing of the
    /// Step's end.
    fn run_step(
        &self,
        request: StepRequest,
    ) -> impl Future<Output = Result<StepReport, StepError>> + Send;

    /// Stop a Step whose Run was cancelled while it worked, or whose work
    /// failed for good.
    fn stop_step(&self, request: StepRequest)
    -> impl Future<Output = Result<(), StepError>> + Send;

    /// Let go of what a Step's work held, once its end is recorded.
    fn release_step(
        &self,
        request: StepRequest,
    ) -> impl Future<Output = Result<(), StepError>> + Send;

    /// Record how a Step ended.
    fn end_step(&self, ending: StepEnding) -> impl Future<Output = Result<(), StepError>> + Send;

    /// Record that a Run waits for a person to answer a Gate.
    fn open_gate(&self, opening: GateOpening)
    -> impl Future<Output = Result<(), StepError>> + Send;

    /// Record the answer a Run's workflow took.
    fn answer_gate(
        &self,
        answered: GateAnswered,
    ) -> impl Future<Output = Result<(), StepError>> + Send;

    /// Ask the Run's Standing Rules about a Gate it opened: record and
    /// return the Rule Answer, or none, for a person to answer it. Asking
    /// again returns the Rule Answer recorded first.
    fn rule_gate(
        &self,
        ruling: GateRuling,
    ) -> impl Future<Output = Result<Option<RuleAnswer>, StepError>> + Send;

    /// Tell the Run's owner that the Gate of the Step `key` waits on them,
    /// when the Run's owner section is on and nothing answered it yet.
    /// Telling them again is the same notification.
    fn notify_gate(&self, key: StepKey) -> impl Future<Output = Result<(), StepError>> + Send;

    /// Tell the Run's owner what a notify-only Gate says, when the Run's
    /// owner section is on, and post it to the task's thread, when its
    /// task thread section is on; the Run goes on either way. Telling them
    /// again is the same notification and the same comment.
    fn tell_gate(&self, telling: GateTelling)
    -> impl Future<Output = Result<(), StepError>> + Send;

    /// Record how a Run ended.
    fn end_run(&self, finish: RunFinish) -> impl Future<Output = Result<(), StepError>> + Send;
}
