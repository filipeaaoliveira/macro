//! The activities a Run's workflow calls, over the Step service.

#[cfg(test)]
mod test;

use std::sync::Arc;
use std::time::Duration;

use temporalio_common::error::ApplicationFailure;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

use crate::domain::gate::{GateAnswered, GateOpening, GateRuling, GateTelling};
use crate::domain::models::{RunFinish, StepEnding, StepError, StepReport, StepRequest, StepStart};
use crate::domain::ports::{ReviewService, StepService};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::domain::standing_rules::RuleAnswer;
use crate::domain::token_budget::{BudgetCheck, BudgetVerdict};
use factory_records::domain::models::{Outcome, StepKey};

/// How often a Step's work tells Temporal its worker is alive. The worker
/// sends them on at most every few seconds (`MAX_HEARTBEAT_THROTTLE` in
/// `inbound::temporal`), and a cancel reaches the work only on the answer to
/// one, so beating more often than that changes nothing.
const HEARTBEAT_EVERY: Duration = Duration::from_secs(2);

/// The Step service as a trait object. `#[activities]` does not take a
/// generic type, so the activities hold the service behind `dyn`, which needs
/// an object-safe (`async_trait`) trait.
#[async_trait::async_trait]
trait DynStepService: Send + Sync {
    async fn start_step(&self, request: StepRequest) -> Result<StepStart, StepError>;

    async fn run_step(&self, request: StepRequest) -> Result<StepReport, StepError>;

    async fn stop_step(&self, request: StepRequest) -> Result<(), StepError>;

    async fn release_step(&self, request: StepRequest) -> Result<(), StepError>;

    async fn end_step(&self, ending: StepEnding) -> Result<(), StepError>;

    async fn open_gate(&self, opening: GateOpening) -> Result<(), StepError>;

    async fn answer_gate(&self, answered: GateAnswered) -> Result<(), StepError>;

    async fn rule_gate(&self, ruling: GateRuling) -> Result<Option<RuleAnswer>, StepError>;

    async fn notify_gate(&self, key: StepKey) -> Result<(), StepError>;

    async fn tell_gate(&self, telling: GateTelling) -> Result<(), StepError>;

    async fn end_run(&self, finish: RunFinish) -> Result<(), StepError>;

    async fn check_budget(&self, check: BudgetCheck) -> Result<BudgetVerdict, StepError>;
}

#[async_trait::async_trait]
impl<T: StepService> DynStepService for T {
    async fn start_step(&self, request: StepRequest) -> Result<StepStart, StepError> {
        StepService::start_step(self, request).await
    }

    async fn run_step(&self, request: StepRequest) -> Result<StepReport, StepError> {
        StepService::run_step(self, request).await
    }

    async fn stop_step(&self, request: StepRequest) -> Result<(), StepError> {
        StepService::stop_step(self, request).await
    }

    async fn release_step(&self, request: StepRequest) -> Result<(), StepError> {
        StepService::release_step(self, request).await
    }

    async fn end_step(&self, ending: StepEnding) -> Result<(), StepError> {
        StepService::end_step(self, ending).await
    }

    async fn open_gate(&self, opening: GateOpening) -> Result<(), StepError> {
        StepService::open_gate(self, opening).await
    }

    async fn answer_gate(&self, answered: GateAnswered) -> Result<(), StepError> {
        StepService::answer_gate(self, answered).await
    }

    async fn rule_gate(&self, ruling: GateRuling) -> Result<Option<RuleAnswer>, StepError> {
        StepService::rule_gate(self, ruling).await
    }

    async fn notify_gate(&self, key: StepKey) -> Result<(), StepError> {
        StepService::notify_gate(self, key).await
    }

    async fn tell_gate(&self, telling: GateTelling) -> Result<(), StepError> {
        StepService::tell_gate(self, telling).await
    }

    async fn end_run(&self, finish: RunFinish) -> Result<(), StepError> {
        StepService::end_run(self, finish).await
    }

    async fn check_budget(&self, check: BudgetCheck) -> Result<BudgetVerdict, StepError> {
        StepService::check_budget(self, check).await
    }
}

/// The review service as a trait object, as [`DynStepService`] is.
#[async_trait::async_trait]
trait DynReviewService: Send + Sync {
    async fn open_round(&self, opening: ReviewRoundOpening) -> Result<bool, StepError>;

    async fn close_round(&self, closing: ReviewRoundClosing) -> Result<Outcome, StepError>;
}

#[async_trait::async_trait]
impl<T: ReviewService> DynReviewService for T {
    async fn open_round(&self, opening: ReviewRoundOpening) -> Result<bool, StepError> {
        ReviewService::open_round(self, opening).await
    }

    async fn close_round(&self, closing: ReviewRoundClosing) -> Result<Outcome, StepError> {
        ReviewService::close_round(self, closing).await
    }
}

/// The factory's activities.
pub struct FactoryActivities {
    steps: Arc<dyn DynStepService>,
    reviews: Arc<dyn DynReviewService>,
}

impl FactoryActivities {
    /// The activities, over `steps` and the review panels' `reviews`.
    pub fn new(steps: impl StepService, reviews: impl ReviewService) -> Self {
        Self {
            steps: Arc::new(steps),
            reviews: Arc::new(reviews),
        }
    }
}

/// The type a Step's work failure carries when it failed for a machine,
/// quota or network reason, for the workflow to end its Run
/// `infrastructure`. Renaming it changes how the Runs that failed so before
/// replay.
pub(crate) const INFRASTRUCTURE_FAILURE: &str = "FactoryInfrastructure";

/// The type a Step's work failure carries when the agent's machine refused
/// the Step's session: not retried, the Run ends `infrastructure`, and its
/// message, the refusal and the machine's reason, is the Step's output.
/// Renaming it changes how the Runs that failed so before replay.
pub(crate) const REFUSED_FAILURE: &str = "FactoryRefused";

/// The type a Step's work failure carries when its agent was still at work
/// when the wait for it ended: retried, and should the Step's time run out
/// on it, the work's failure, never infrastructure's. Renaming it changes
/// how the Runs that failed so before replay.
pub(crate) const AGENT_OVERDUE_FAILURE: &str = "FactoryAgentOverdue";

/// The type a Step's work failure carries when every Machine its Agents may
/// run on stayed full: retried, and should the Step's time run out on it,
/// infrastructure's, never the work's. Renaming it changes how the Runs that
/// failed so before replay.
pub(crate) const WAITING_FOR_MACHINE_FAILURE: &str = "FactoryWaitingForMachine";

/// A Step error as Temporal sees it: retried unless the same Step would fail
/// the same way again.
pub(super) fn failure(error: StepError) -> ActivityError {
    match error {
        StepError::OutOfQuota(_) => ApplicationFailure::builder(error)
            .type_name(INFRASTRUCTURE_FAILURE.to_string())
            .non_retryable(true)
            .build()
            .into(),
        StepError::Refused(_) => ApplicationFailure::builder(error)
            .type_name(REFUSED_FAILURE.to_string())
            .non_retryable(true)
            .build()
            .into(),
        StepError::Overdue(_) => ApplicationFailure::builder(error)
            .type_name(AGENT_OVERDUE_FAILURE.to_string())
            .build()
            .into(),
        StepError::OutputTooLarge { .. }
        | StepError::NoSuchRun
        | StepError::RunEnded
        | StepError::Final(_) => ApplicationFailure::non_retryable(error).into(),
        StepError::WaitingForMachine { last, .. } => ApplicationFailure::builder(error)
            .type_name(WAITING_FOR_MACHINE_FAILURE.to_string())
            .non_retryable(last)
            .build()
            .into(),
        StepError::Internal(_) => ApplicationFailure::new(error).into(),
    }
}

/// What a Step's work needs of its activity's context: to hear that the
/// activity was cancelled, and to tell Temporal the worker is alive. The
/// tests stand one in for Temporal's, since the SDK's `ActivityEnvironment`
/// needs its `testing` feature, whose test server workspace-hack can't unify.
trait Liveness: Sync {
    /// Resolves once the activity is cancelled.
    fn cancelled(&self) -> impl Future<Output = ()> + Send;

    /// Tell Temporal the worker is alive.
    fn beat(&self) -> impl Future<Output = ()> + Send;
}

impl Liveness for ActivityContext {
    fn cancelled(&self) -> impl Future<Output = ()> + Send {
        ActivityContext::cancelled(self)
    }

    async fn beat(&self) {
        if let Err(error) = self.record_heartbeat(()).await {
            tracing::warn!(?error, "a step's heartbeat was not recorded");
        }
    }
}

/// Run `work`, telling Temporal the worker is alive while it goes. When the
/// activity is cancelled, drop the work, so nothing more of it is sent, and
/// report the cancellation. The activity does not see why it was cancelled:
/// the Run's cancellation, the worker shutting down, or a timeout that
/// already handed the Step to another attempt all look the same, so nothing
/// the work holds is touched here.
async fn beating<T>(
    live: &impl Liveness,
    work: impl Future<Output = Result<T, StepError>>,
) -> Result<T, ActivityError> {
    let mut work = std::pin::pin!(work);
    let mut beat = tokio::time::interval(HEARTBEAT_EVERY);
    loop {
        tokio::select! {
            done = &mut work => return done.map_err(failure),
            () = live.cancelled() => return Err(ActivityError::cancelled()),
            _ = beat.tick() => live.beat().await,
        }
    }
}

impl FactoryActivities {
    /// A Step's work, as [`Self::run_step`] does it. Cancelled, it drops the
    /// work and leaves the Step's agent be.
    async fn work(
        &self,
        live: &impl Liveness,
        request: StepRequest,
    ) -> Result<StepReport, ActivityError> {
        beating(live, self.steps.run_step(request)).await
    }

    /// Stopping a Step's agent, as [`Self::stop_step`] does it.
    async fn stop(&self, live: &impl Liveness, request: StepRequest) -> Result<(), ActivityError> {
        beating(live, self.steps.stop_step(request)).await
    }
}

#[activities]
impl FactoryActivities {
    /// Record that a Step started, unless its Run already ended.
    #[activity(name = "factory_start_step")]
    pub async fn start_step(
        self: Arc<Self>,
        _ctx: ActivityContext,
        request: StepRequest,
    ) -> Result<StepStart, ActivityError> {
        self.steps.start_step(request).await.map_err(failure)
    }

    /// Do a Step's work, heartbeating while it goes. Cancelled, it drops the
    /// work and leaves the Step's agent be: a retry goes on with it, and the
    /// workflow stops it when the Run was cancelled.
    #[activity(name = "factory_run_step")]
    pub async fn run_step(
        self: Arc<Self>,
        ctx: ActivityContext,
        request: StepRequest,
    ) -> Result<StepReport, ActivityError> {
        self.work(&ctx, request).await
    }

    /// Stop the agent of a Step whose Run was cancelled, or whose work
    /// failed for good, so nothing that follows the Step runs beside it.
    #[activity(name = "factory_stop_step")]
    pub async fn stop_step(
        self: Arc<Self>,
        ctx: ActivityContext,
        request: StepRequest,
    ) -> Result<(), ActivityError> {
        self.stop(&ctx, request).await
    }

    /// Let go of what a Step's work held, once its end is recorded.
    #[activity(name = "factory_release_step")]
    pub async fn release_step(
        self: Arc<Self>,
        _ctx: ActivityContext,
        request: StepRequest,
    ) -> Result<(), ActivityError> {
        self.steps.release_step(request).await.map_err(failure)
    }

    /// Record how a Step ended.
    #[activity(name = "factory_end_step")]
    pub async fn end_step(
        self: Arc<Self>,
        _ctx: ActivityContext,
        ending: StepEnding,
    ) -> Result<(), ActivityError> {
        self.steps.end_step(ending).await.map_err(failure)
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

    /// Ask the Run's Standing Rules about a Gate it opened, and record the
    /// Rule Answer they give, if any.
    #[activity(name = "factory_rule_gate")]
    pub async fn rule_gate(
        self: Arc<Self>,
        _ctx: ActivityContext,
        ruling: GateRuling,
    ) -> Result<Option<RuleAnswer>, ActivityError> {
        self.steps.rule_gate(ruling).await.map_err(failure)
    }

    /// Tell the Run's owner that a Gate no Standing Rule answered waits on
    /// them, unless the Run's definition is quiet.
    #[activity(name = "factory_notify_gate")]
    pub async fn notify_gate(
        self: Arc<Self>,
        _ctx: ActivityContext,
        key: StepKey,
    ) -> Result<(), ActivityError> {
        self.steps.notify_gate(key).await.map_err(failure)
    }

    /// Tell the Run's owner what a notify-only Gate says, and post it to
    /// the task's thread, as the Run's sections say; the Run goes on.
    #[activity(name = "factory_tell_gate")]
    pub async fn tell_gate(
        self: Arc<Self>,
        _ctx: ActivityContext,
        telling: GateTelling,
    ) -> Result<(), ActivityError> {
        self.steps.tell_gate(telling).await.map_err(failure)
    }

    /// Record the review round a review panel starts, before its
    /// reviewers' Steps.
    #[activity(name = "factory_open_review_round")]
    pub async fn open_review_round(
        self: Arc<Self>,
        _ctx: ActivityContext,
        opening: ReviewRoundOpening,
    ) -> Result<bool, ActivityError> {
        self.reviews.open_round(opening).await.map_err(failure)
    }

    /// End a review round once its reviewers' Steps ended, and say how it
    /// ended: its Blockers told from its follow-ups, which become tasks.
    /// It heartbeats while it goes, however many tasks it makes.
    #[activity(name = "factory_close_review_round")]
    pub async fn close_review_round(
        self: Arc<Self>,
        ctx: ActivityContext,
        closing: ReviewRoundClosing,
    ) -> Result<Outcome, ActivityError> {
        beating(&ctx, self.reviews.close_round(closing)).await
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

    /// Check a Run's token budget before its next agent Step, and keep why
    /// it ends the Run when it does.
    #[activity(name = "factory_check_budget")]
    pub async fn check_budget(
        self: Arc<Self>,
        _ctx: ActivityContext,
        check: BudgetCheck,
    ) -> Result<BudgetVerdict, ActivityError> {
        self.steps.check_budget(check).await.map_err(failure)
    }
}
