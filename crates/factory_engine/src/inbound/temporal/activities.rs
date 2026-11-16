//! The activities a Run's workflow calls, over the Step service.

use std::sync::Arc;

use temporalio_common::error::ApplicationFailure;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

use crate::domain::gate::{GateAnswered, GateOpening};
use crate::domain::models::{RunFinish, StepError, StepReport, StepRequest};
use crate::domain::ports::StepService;

/// The Step service as a trait object. `#[activities]` does not take a
/// generic type, so the activities hold the service behind `dyn`, which needs
/// an object-safe (`async_trait`) trait.
#[async_trait::async_trait]
trait DynStepService: Send + Sync {
    async fn run_step(&self, request: StepRequest) -> Result<StepReport, StepError>;

    async fn open_gate(&self, opening: GateOpening) -> Result<(), StepError>;

    async fn answer_gate(&self, answered: GateAnswered) -> Result<(), StepError>;

    async fn end_run(&self, finish: RunFinish) -> Result<(), StepError>;
}

#[async_trait::async_trait]
impl<T: StepService> DynStepService for T {
    async fn run_step(&self, request: StepRequest) -> Result<StepReport, StepError> {
        StepService::run_step(self, request).await
    }

    async fn open_gate(&self, opening: GateOpening) -> Result<(), StepError> {
        StepService::open_gate(self, opening).await
    }

    async fn answer_gate(&self, answered: GateAnswered) -> Result<(), StepError> {
        StepService::answer_gate(self, answered).await
    }

    async fn end_run(&self, finish: RunFinish) -> Result<(), StepError> {
        StepService::end_run(self, finish).await
    }
}

/// The factory's activities.
pub struct FactoryActivities {
    steps: Arc<dyn DynStepService>,
}

impl FactoryActivities {
    /// The activities, over `steps`.
    pub fn new(steps: impl StepService) -> Self {
        Self {
            steps: Arc::new(steps),
        }
    }
}

/// A Step error as Temporal sees it: retried unless the same Step would fail
/// the same way again.
fn failure(error: StepError) -> ActivityError {
    match error {
        StepError::OutputTooLarge { .. } => ApplicationFailure::non_retryable(error).into(),
        StepError::Internal(_) => ApplicationFailure::new(error).into(),
    }
}

#[activities]
impl FactoryActivities {
    /// Run and record one Step.
    #[activity(name = "factory_run_step")]
    pub async fn run_step(
        self: Arc<Self>,
        _ctx: ActivityContext,
        request: StepRequest,
    ) -> Result<StepReport, ActivityError> {
        self.steps.run_step(request).await.map_err(failure)
    }

    /// Record that a Run waits for a person to answer a Gate.
    #[activity(name = "factory_open_gate")]
    pub async fn open_gate(
        self: Arc<Self>,
        _ctx: ActivityContext,
        opening: GateOpening,
    ) -> Result<(), ActivityError> {
        self.steps.open_gate(opening).await.map_err(failure)
    }

    /// Record the answer a Run's workflow took.
    #[activity(name = "factory_answer_gate")]
    pub async fn answer_gate(
        self: Arc<Self>,
        _ctx: ActivityContext,
        answered: GateAnswered,
    ) -> Result<(), ActivityError> {
        self.steps.answer_gate(answered).await.map_err(failure)
    }

    /// Record how a Run ended.
    #[activity(name = "factory_end_run")]
    pub async fn end_run(
        self: Arc<Self>,
        _ctx: ActivityContext,
        finish: RunFinish,
    ) -> Result<(), ActivityError> {
        self.steps.end_run(finish).await.map_err(failure)
    }
}
