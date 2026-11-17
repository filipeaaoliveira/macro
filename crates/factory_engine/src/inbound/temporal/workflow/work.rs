//! What the workflow asks of Temporal, as one trait ([`Work`]): the walk of
//! a Step and of a review round are written over it, so the workflow runs
//! them on its context ([`TemporalWork`]) and the tests on stand-ins,
//! without a Temporal server. Either way a walk issues the same activities
//! in the same order, so a replay walks it the same way.

use factory_records::domain::models::Outcome;
use temporalio_sdk::workflows::join_all;
use temporalio_sdk::{
    ActivityCloseTimeouts, ActivityOptions, WorkflowContext, WorkflowTermination,
};

use super::{
    CLOSE_ROUND_TIMEOUT, CLOSE_ROUND_TOTAL, FlowWorkflow, OPEN_ROUND_TIMEOUT,
    STEP_HEARTBEAT_TIMEOUT, ask, is_cancelled, stop,
};
use crate::domain::gate::{AnswerGate, GateOpening};
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::inbound::temporal::activities::FactoryActivities;

/// What a Step's walk and a review round ask of Temporal: their
/// activities, the Run's Gates, and whether the Run was cancelled.
pub(super) trait Work {
    /// Why an activity failed for good.
    type Failure;

    /// Whether the Run's workflow was cancelled.
    fn cancelled(&self) -> bool;

    /// Open the round: whether it opened.
    async fn open(&self, opening: ReviewRoundOpening) -> Result<bool, Self::Failure>;

    /// Record that a Step started.
    async fn start(&self, request: StepRequest) -> Result<StepStart, Self::Failure>;

    /// Run one Step's work.
    async fn run(&self, request: &StepRequest) -> Result<StepReport, Self::Failure>;

    /// Run the reviewers' work at once, and return each one's result in
    /// the order of `requests`.
    async fn run_all(&self, requests: &[StepRequest]) -> Vec<Result<StepReport, Self::Failure>>;

    /// Record how a Step ended.
    async fn end(&self, ending: StepEnding) -> Result<(), Self::Failure>;

    /// Let go of a Step's session once its end is recorded.
    async fn release(&self, request: StepRequest) -> Result<(), Self::Failure>;

    /// Stop a Step's agent for good.
    async fn stop(&self, request: &StepRequest);

    /// Open the Gate `opening`, wait for a person's answer, record it, and
    /// return it.
    async fn ask(&self, opening: GateOpening) -> Result<AnswerGate, Self::Failure>;

    /// Close the round, and return its outcome.
    async fn close(&self, closing: ReviewRoundClosing) -> Result<Outcome, Self::Failure>;
}

/// The activities and Gates on the workflow's context.
pub(super) struct TemporalWork<'a> {
    pub(super) ctx: &'a WorkflowContext<FlowWorkflow>,
    pub(super) work_options: &'a ActivityOptions,
    pub(super) record_options: &'a ActivityOptions,
}

impl Work for TemporalWork<'_> {
    type Failure = WorkflowTermination;

    fn cancelled(&self) -> bool {
        is_cancelled(self.ctx)
    }

    async fn open(&self, opening: ReviewRoundOpening) -> Result<bool, Self::Failure> {
        Ok(self
            .ctx
            .execute_activity(
                FactoryActivities::open_review_round,
                opening,
                ActivityOptions::start_to_close_timeout(OPEN_ROUND_TIMEOUT),
            )
            .await?)
    }

    async fn start(&self, request: StepRequest) -> Result<StepStart, Self::Failure> {
        Ok(self
            .ctx
            .execute_activity(
                FactoryActivities::start_step,
                request,
                self.record_options.clone(),
            )
            .await?)
    }

    async fn run(&self, request: &StepRequest) -> Result<StepReport, Self::Failure> {
        Ok(self
            .ctx
            .execute_activity(
                FactoryActivities::run_step,
                request.clone(),
                self.work_options.clone(),
            )
            .await?)
    }

    async fn run_all(&self, requests: &[StepRequest]) -> Vec<Result<StepReport, Self::Failure>> {
        // The activities are scheduled in the entry's order and their
        // results read in it, so a replay walks the same way.
        join_all(requests.iter().map(|request| {
            self.ctx.execute_activity(
                FactoryActivities::run_step,
                request.clone(),
                self.work_options.clone(),
            )
        }))
        .await
        .into_iter()
        .map(|worked| worked.map_err(Into::into))
        .collect()
    }

    async fn end(&self, ending: StepEnding) -> Result<(), Self::Failure> {
        Ok(self
            .ctx
            .execute_activity(
                FactoryActivities::end_step,
                ending,
                self.record_options.clone(),
            )
            .await?)
    }

    async fn release(&self, request: StepRequest) -> Result<(), Self::Failure> {
        Ok(self
            .ctx
            .execute_activity(
                FactoryActivities::release_step,
                request,
                self.record_options.clone(),
            )
            .await?)
    }

    async fn stop(&self, request: &StepRequest) {
        stop(self.ctx, request).await;
    }

    async fn ask(&self, opening: GateOpening) -> Result<AnswerGate, Self::Failure> {
        ask(self.ctx, opening).await
    }

    async fn close(&self, closing: ReviewRoundClosing) -> Result<Outcome, Self::Failure> {
        // Bounded across its attempts, so an outage that never passes ends
        // the round failed, for a person, rather than hold the Run.
        let options =
            ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::ScheduleAndStartToClose {
                schedule_to_close: CLOSE_ROUND_TOTAL,
                start_to_close: CLOSE_ROUND_TIMEOUT,
            })
            .heartbeat_timeout(STEP_HEARTBEAT_TIMEOUT)
            .build();
        Ok(self
            .ctx
            .execute_activity(FactoryActivities::close_review_round, closing, options)
            .await?)
    }
}
