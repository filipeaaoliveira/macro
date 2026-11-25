//! What the workflow asks of Temporal, as one trait ([`Work`]): the walk of
//! a Step and of a review round are written over it, so the workflow runs
//! them on its context ([`TemporalWork`]) and the tests on stand-ins,
//! without a Temporal server. Either way a walk issues the same activities
//! in the same order, so a replay walks it the same way.

use factory_records::domain::models::{Outcome, RunId};
use temporalio_common::error::{ActivityExecutionError, IncomingError, TimeoutError, TimeoutType};
use temporalio_sdk::workflows::join_all;
use temporalio_sdk::{
    ActivityCloseTimeouts, ActivityOptions, WorkflowContext, WorkflowTermination,
};

use super::{
    Asking, CLOSE_ROUND_TIMEOUT, CLOSE_ROUND_TOTAL, FAILED_STEP_OUTPUT, FlowWorkflow,
    NO_RESULT_PATCH, NOTIFY_ONLY_PATCH, OPEN_ROUND_TIMEOUT, STEP_HEARTBEAT_TIMEOUT, ask,
    is_cancelled, notify_options, stop, within_budget,
};
use crate::domain::gate::{GateOpening, GateTelling};
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::inbound::temporal::activities::{
    FactoryActivities, INFRASTRUCTURE_FAILURE, REFUSED_FAILURE, WAITING_FOR_MACHINE_FAILURE,
};

/// Why a Step's work failed for good, as the workflow reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WorkFailure {
    /// For a machine, quota or network reason: its agents' quota ran out,
    /// or its attempts ran out of time as their worker was lost.
    Infrastructure,
    /// For infrastructure too: the agent's machine refused the Step's
    /// session. It holds what the Run's owner is shown: the refusal and
    /// the machine's reason.
    Refused(String),
    /// For the work itself, as an agent that ended the Step with an
    /// outcome it does not offer.
    Work,
}

impl WorkFailure {
    /// Whether the Step failed for a machine, quota or network reason.
    pub(super) fn infrastructure(&self) -> bool {
        matches!(self, WorkFailure::Infrastructure | WorkFailure::Refused(_))
    }

    /// What the Run's owner is shown of the Step that failed so.
    pub(super) fn output(&self) -> String {
        match self {
            WorkFailure::Refused(refusal) => refusal.clone(),
            WorkFailure::Infrastructure | WorkFailure::Work => FAILED_STEP_OUTPUT.to_string(),
        }
    }
}

/// Why the activity of a Step's work failed for good, read from its
/// failure, which the workflow's history keeps, so a replay reads it the
/// same way. Only what is surely the factory's counts as infrastructure;
/// when unsure, the failure is the work's. Infrastructure's are: agents out
/// of quota (`FactoryInfrastructure`), a Step whose Machines stayed full
/// (`FactoryWaitingForMachine`, a type no history before it carries), and timeouts no slow agent explains
/// ([`timeout_failure`]). Attempts whose time ran out after a failure
/// Temporal retried come back as that last failure, not as a timeout: one
/// of no known kind, as an agent still at work (`FactoryAgentOverdue`) or a
/// round of checks that could not go on, is the work's. A session the
/// agent's machine refused (`FactoryRefused`) is infrastructure's, and
/// keeps the refusal for the Step's output. This decides
/// whether the workflow asks for the `factory-infrastructure-outcome`
/// marker, so changing it changes how recorded histories replay.
pub(super) fn work_failure(error: &ActivityExecutionError) -> WorkFailure {
    if let Some(timeout) = error.as_timeout() {
        return timeout_failure(timeout);
    }
    match error.cause() {
        Some(cause) => known_failure(cause).unwrap_or(WorkFailure::Work),
        None => WorkFailure::Work,
    }
}

/// What `failure` is when the factory knows its kind: agents out of quota,
/// or a session their machine refused.
fn known_failure(failure: &IncomingError) -> Option<WorkFailure> {
    if out_of_quota(failure) {
        return Some(WorkFailure::Infrastructure);
    }
    match failure {
        IncomingError::Application(application)
            if application.type_name() == Some(REFUSED_FAILURE) =>
        {
            Some(WorkFailure::Refused(
                application
                    .failure()
                    .map(|failure| failure.message.clone())
                    .unwrap_or_else(|| FAILED_STEP_OUTPUT.to_string()),
            ))
        }
        _ => None,
    }
}

/// Whether `failure` is agents out of quota, or a Step whose Machines
/// stayed full: either way infrastructure's.
fn out_of_quota(failure: &IncomingError) -> bool {
    match failure {
        IncomingError::Application(failure) => {
            failure.type_name() == Some(INFRASTRUCTURE_FAILURE)
                || failure.type_name() == Some(WAITING_FOR_MACHINE_FAILURE)
        }
        _ => false,
    }
}

/// Why a Step's work timed out. Its worker went quiet (`Heartbeat`), or no
/// worker took it (`ScheduleToStart`): infrastructure's. An attempt that
/// ran past its own time (`StartToClose`) may be an agent slower than its
/// deadline: the work's. A Step whose whole time ran out (`ScheduleToClose`)
/// reads as its last attempt did, and as infrastructure's when none left a
/// failure.
fn timeout_failure(timeout: &TimeoutError) -> WorkFailure {
    match timeout.timeout_type() {
        TimeoutType::Heartbeat | TimeoutType::ScheduleToStart => WorkFailure::Infrastructure,
        TimeoutType::ScheduleToClose => match timeout.cause() {
            None => WorkFailure::Infrastructure,
            Some(IncomingError::Timeout(last)) => timeout_failure(last),
            Some(last) => known_failure(last).unwrap_or(WorkFailure::Work),
        },
        // `StartToClose`, and any kind this code does not know: unsure.
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

    /// Whether the Run `run_id` may take its next agent Step: false once
    /// its token budget stopped it.
    async fn within_budget(&self, run_id: RunId) -> bool;

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
    /// Whether the Run keeps a token budget to check.
    pub(super) token_budget: bool,
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

    async fn within_budget(&self, run_id: RunId) -> bool {
        within_budget(self.ctx, run_id, self.token_budget).await
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
