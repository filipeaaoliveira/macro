//! What the workflow asks of Temporal, as one trait ([`Work`]): the walk of
//! a Step and of a review round are written over it, so the workflow runs
//! them on its context ([`TemporalWork`]) and the tests on stand-ins,
//! without a Temporal server. Either way a walk issues the same activities
//! in the same order, so a replay walks it the same way.

use factory_records::domain::models::Outcome;
use temporalio_common::error::{ActivityExecutionError, IncomingError};
use temporalio_sdk::workflows::join_all;
use temporalio_sdk::{
    ActivityCloseTimeouts, ActivityOptions, WorkflowContext, WorkflowTermination,
};

use super::{
    Asking, CLOSE_ROUND_TIMEOUT, CLOSE_ROUND_TOTAL, FlowWorkflow, NO_RESULT_PATCH,
    NOTIFY_ONLY_PATCH, OPEN_ROUND_TIMEOUT, STEP_HEARTBEAT_TIMEOUT, ask, is_cancelled,
    notify_options, stop,
};
use crate::domain::gate::{GateOpening, GateTelling};
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::inbound::temporal::activities::{FactoryActivities, INFRASTRUCTURE_FAILURE};

/// Why a Step's work failed for good, as the workflow reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkFailure {
    /// For a machine, quota or network reason: its agents' quota ran out,
    /// or its attempts ran out of time as their worker was lost.
    Infrastructure,
    /// For the work itself, as an agent that ended the Step with an
    /// outcome it does not offer.
    Work,
}

/// Why the activity of a Step's work failed for good, read from its
/// failure, which the workflow's history keeps, so a replay reads it the
/// same way. Only what is surely the factory's counts as infrastructure;
/// when unsure, the failure is the work's. Infrastructure's are: agents out
/// of quota (`FactoryInfrastructure`), and a timeout whose last attempt
/// failed so too, timed out itself (its worker was lost) or left no
/// failure. Attempts whose time ran out after a failure Temporal retried
/// come back as that last failure, not as a timeout: one of no known kind,
/// as an agent still at work (`FactoryAgentOverdue`) or a round of checks
/// that could not go on, is the work's. This decides whether the
/// workflow asks for the `factory-infrastructure-outcome` marker, so
/// changing it changes how recorded histories replay.
pub(super) fn work_failure(error: &ActivityExecutionError) -> WorkFailure {
    let infrastructure = |failure: &IncomingError| match failure {
        IncomingError::Application(failure) => failure.type_name() == Some(INFRASTRUCTURE_FAILURE),
        _ => false,
    };
    if let Some(timeout) = error.as_timeout() {
        return match timeout.cause() {
            None | Some(IncomingError::Timeout(_)) => WorkFailure::Infrastructure,
            Some(last) if infrastructure(last) => WorkFailure::Infrastructure,
            Some(_) => WorkFailure::Work,
        };
    }
    match error.cause() {
        Some(cause) if infrastructure(cause) => WorkFailure::Infrastructure,
        _ => WorkFailure::Work,
    }
}

/// What a Step's walk and a review round ask of Temporal: their
/// activities, the Run's Gates, and whether the Run was cancelled.
pub(super) trait Work {
    /// Why an activity failed for good.
    type Failure;

    /// Whether the Run's workflow was cancelled.
    fn cancelled(&self) -> bool;

    /// Whether a Step whose agent stopped without a result asks the Run's
    /// owner, rather than fail: false only for a Run whose history passed
    /// such a Step before the workflow asked.
    fn asks_on_no_result(&self) -> bool;

    /// Open the round: whether it opened.
    async fn open(&self, opening: ReviewRoundOpening) -> Result<bool, Self::Failure>;

    /// Record that a Step started.
    async fn start(&self, request: StepRequest) -> Result<StepStart, Self::Failure>;

    /// Run one Step's work, or say why it failed for good.
    async fn run(&self, request: &StepRequest) -> Result<StepReport, WorkFailure>;

    /// Run the reviewers' work at once, and return each one's result in
    /// the order of `requests`.
    async fn run_all(&self, requests: &[StepRequest]) -> Vec<Result<StepReport, WorkFailure>>;

    /// Record how a Step ended.
    async fn end(&self, ending: StepEnding) -> Result<(), Self::Failure>;

    /// Let go of a Step's session once its end is recorded.
    async fn release(&self, request: StepRequest) -> Result<(), Self::Failure>;

    /// Stop a Step's agent for good.
    async fn stop(&self, request: &StepRequest);

    /// Open the Gate `opening`; for a Flow's Gate (`rule`), ask the Run's
    /// Standing Rules about it and return their Rule Answer; otherwise, or
    /// when no rule answers, wait for a person's answer, record it, and
    /// return it. A question Gate has no `rule`: only a person answers it.
    async fn ask(
        &self,
        opening: GateOpening,
        rule: Option<Asking>,
    ) -> Result<Outcome, Self::Failure>;

    /// Tell the Run's owner what a notify-only Gate says, without waiting
    /// on them. Telling them may fail: the Run goes on all the same.
    async fn tell(&self, telling: GateTelling);

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

    fn asks_on_no_result(&self) -> bool {
        self.ctx.patched(NO_RESULT_PATCH)
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

    async fn run(&self, request: &StepRequest) -> Result<StepReport, WorkFailure> {
        self.ctx
            .execute_activity(
                FactoryActivities::run_step,
                request.clone(),
                self.work_options.clone(),
            )
            .await
            .map_err(|error| work_failure(&error))
    }

    async fn run_all(&self, requests: &[StepRequest]) -> Vec<Result<StepReport, WorkFailure>> {
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
        .map(|worked| worked.map_err(|error| work_failure(&error)))
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

    async fn ask(
        &self,
        opening: GateOpening,
        rule: Option<Asking>,
    ) -> Result<Outcome, Self::Failure> {
        ask(self.ctx, opening, rule).await
    }

    async fn tell(&self, telling: GateTelling) {
        // Only a Flow with a notify-only Gate comes here, and no Run in
        // flight before notify-only Gates carries one; the marker keeps
        // the rule that every new command of the workflow is patched in.
        if self.ctx.patched(NOTIFY_ONLY_PATCH) {
            let _ = self
                .ctx
                .execute_activity(FactoryActivities::tell_gate, telling, notify_options())
                .await;
        }
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

#[cfg(test)]
mod test;
