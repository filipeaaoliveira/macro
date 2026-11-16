//! Ports (trait contracts) of the factory's use cases.

use factory_records::domain::models::RunId;

use crate::domain::flow::StepName;
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening};
use crate::domain::models::{
    AgentAnswer, AgentTask, FactoryError, RunDetail, RunFinish, RunStart, StepError, StepInput,
    StepReport, StepRequest,
};

/// What a Run's workflow did with an answer it was sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerDelivery {
    /// The workflow took the answer; the Run goes on.
    Taken,
    /// The workflow refused the answer, for this reason.
    Refused(String),
}

/// Starts, answers and cancels the workflows that walk Runs.
pub trait RunWorkflows: Send + Sync + 'static {
    /// The error type returned when the workflow cannot be reached.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Start the workflow of `start.run_id`. Starting a Run that already
    /// started changes nothing.
    fn start(&self, start: &RunStart) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Send `answer` to the workflow of `run_id`, which takes it or says why
    /// not.
    fn answer(
        &self,
        run_id: RunId,
        answer: &AnswerGate,
    ) -> impl Future<Output = Result<AnswerDelivery, Self::Err>> + Send;

    /// Ask the workflow of `run_id` to stop its Step or Gate and end the Run
    /// cancelled. Asking again, or once the workflow ended, changes nothing.
    fn cancel(&self, run_id: RunId) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// Does the work of a Flow's Steps.
pub trait StepExecutor: Send + Sync + 'static {
    /// The error type returned when a Step's work fails.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Whether it does `step`'s work.
    fn serves(&self, step: &StepName) -> bool;

    /// Do `step`'s work on `input`. A Temporal retry may call it again for the
    /// same Step.
    fn run(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> impl Future<Output = Result<StepReport, Self::Err>> + Send;
}

/// An agent that does a Step's work: it reads the task's prompt, works, and
/// ends with one of the task's outcomes.
pub trait StepAgent: Send + Sync + 'static {
    /// The error type returned when the agent cannot do the task.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Do `task` and hand back how it ended. A Temporal retry may ask again
    /// for the same Step.
    fn answer(
        &self,
        task: AgentTask,
    ) -> impl Future<Output = Result<AgentAnswer, Self::Err>> + Send;
}

/// Inbound service port: the Run API used by drivers (HTTP).
pub trait FactoryService: Send + Sync + 'static {
    /// Record a new Run of the Flow `definition` for `request`, and start it.
    fn create_run(
        &self,
        definition: serde_json::Value,
        request: String,
    ) -> impl Future<Output = Result<RunDetail, FactoryError>> + Send;

    /// The Run and the Steps it took.
    fn get_run(&self, id: RunId) -> impl Future<Output = Result<RunDetail, FactoryError>> + Send;

    /// Give a person's answer to the Gate the Run waits on.
    fn answer_gate(
        &self,
        id: RunId,
        answer: AnswerGate,
    ) -> impl Future<Output = Result<(), FactoryError>> + Send;

    /// Cancel a Run that has not ended.
    fn cancel_run(&self, id: RunId) -> impl Future<Output = Result<(), FactoryError>> + Send;
}

/// Inbound service port: what a Run's workflow asks its worker to do.
pub trait StepService: Send + Sync + 'static {
    /// Run one Step and record it.
    fn run_step(
        &self,
        request: StepRequest,
    ) -> impl Future<Output = Result<StepReport, StepError>> + Send;

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
