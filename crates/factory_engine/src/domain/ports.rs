//! Ports (trait contracts) of the factory's use cases.

use factory_records::domain::models::RunId;

use crate::domain::flow::StepName;
use crate::domain::models::{
    FactoryError, RunDetail, RunFinish, RunStart, StepError, StepReport, StepRequest,
};

/// Starts a Run's workflow.
pub trait RunScheduler: Send + Sync + 'static {
    /// The error type returned when the workflow does not start.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Start the workflow of `start.run_id`. Starting a Run that already
    /// started changes nothing.
    fn start(&self, start: &RunStart) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// Does the work of a Flow's Steps.
pub trait StepExecutor: Send + Sync + 'static {
    /// The error type returned when a Step's work fails.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Whether it does `step`'s work.
    fn serves(&self, step: &StepName) -> bool;

    /// Do `step`'s work for `request`. A Temporal retry may call it again for
    /// the same Step.
    fn run(
        &self,
        step: &StepName,
        request: &str,
    ) -> impl Future<Output = Result<StepReport, Self::Err>> + Send;
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
}

/// Inbound service port: what a Run's workflow asks its worker to do.
pub trait StepService: Send + Sync + 'static {
    /// Run one Step and record it.
    fn run_step(
        &self,
        request: StepRequest,
    ) -> impl Future<Output = Result<StepReport, StepError>> + Send;

    /// Record how a Run ended.
    fn end_run(&self, finish: RunFinish) -> impl Future<Output = Result<(), StepError>> + Send;
}
