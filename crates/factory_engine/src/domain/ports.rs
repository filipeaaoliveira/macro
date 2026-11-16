//! Ports (trait contracts) of the factory's use cases.

use agent_session::domain::session_task::TaskDocumentId;
use factory_records::domain::models::{HandBack, Outcome, RunId, StepKey};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::flow::StepName;
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening};
use crate::domain::models::{
    AgentAnswer, AgentTask, FactoryError, FlowSummary, RunDetail, RunFinish, RunStart, StepEnding,
    StepError, StepInput, StepReport, StepRequest, StepStart,
};

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

/// Does the work of a Flow's Steps.
pub trait StepExecutor: Send + Sync + 'static {
    /// The error type returned when a Step's work fails.
    type Err: std::error::Error + Retryable + Send + Sync + 'static;

    /// Whether it does `step`'s work.
    fn serves(&self, step: &StepName) -> bool;

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
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Let go of what the work of the Step `key` held, once its end is
    /// recorded. No retry of its work comes after that.
    fn release(
        &self,
        step: &StepName,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// An agent that does a Step's work: it reads the task's prompt, works, and
/// ends with one of the task's outcomes.
pub trait StepAgent: Send + Sync + 'static {
    /// The error type returned when the agent cannot do the task.
    type Err: std::error::Error + Retryable + Send + Sync + 'static;

    /// Do `task` and hand back how it ended. A Temporal retry may ask again
    /// for the same Step: the agent goes on with it, never starts it twice.
    fn answer(
        &self,
        task: AgentTask,
    ) -> impl Future<Output = Result<AgentAnswer, Self::Err>> + Send;

    /// Stop working on the Step `key` for `owner`, for good.
    fn stop(
        &self,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Let go of the Step `key`'s work for `owner`, which ended and whose end
    /// is recorded.
    fn release(
        &self,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
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
}

/// Inbound service port: the Run API used by drivers (HTTP). Every call but
/// a hand back is made for an acting user, and reaches only that user's Runs.
pub trait FactoryService: Send + Sync + 'static {
    /// The built-in Flows a Run can be started of: those that name only
    /// Steps the worker serves.
    fn list_flows(&self) -> Vec<FlowSummary>;

    /// Record `actor`'s new Run of the built-in Flow named `flow` for
    /// `request`, working on the task document `task_id` if one is given,
    /// and start it. The actor must have an account and see the task.
    fn create_run(
        &self,
        actor: MacroUserIdStr<'static>,
        flow: String,
        request: String,
        task_id: Option<Uuid>,
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

    /// Link `actor`'s Run to the task document `task_id`, replacing any task
    /// it had. The actor must see the task.
    fn link_task(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
        task_id: Uuid,
    ) -> impl Future<Output = Result<RunDetail, FactoryError>> + Send;

    /// Give `actor`'s answer to the Gate the actor's Run waits on.
    fn answer_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        id: RunId,
        seq: u16,
        answer: Outcome,
        note: Option<String>,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Keep what the agent working in the session `session_id`, acting as
    /// `user_id`, hands back for its Step, replacing an earlier hand back.
    fn hand_back(
        &self,
        session_id: Uuid,
        user_id: MacroUserIdStr<'static>,
        hand_back: HandBack,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Cancel `actor`'s Run. Cancelling a Run that ended cancelled changes
    /// nothing.
    fn cancel_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;
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

    /// Record how a Run ended.
    fn end_run(&self, finish: RunFinish) -> impl Future<Output = Result<(), StepError>> + Send;
}
