//! A review panel's visit: one review round. The round's walk is written
//! over [`RoundWork`], what it asks of Temporal, so the workflow runs it on
//! its context ([`TemporalRound`]) and the tests on a stand-in, without a
//! Temporal server. The walk issues the same activities in the same order
//! either way, so a replay walks it the same way.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, RunId, RunOutcome, StepKey};
use temporalio_sdk::ActivityExecutionError;
use temporalio_sdk::workflows::join_all;
use temporalio_sdk::{ActivityCloseTimeouts, ActivityOptions, WorkflowContext};

use super::{
    CLOSE_ROUND_TIMEOUT, FAILED_STEP_OUTPUT, FlowWorkflow, OPEN_ROUND_TIMEOUT,
    STEP_HEARTBEAT_TIMEOUT, is_cancelled, stop,
};
use crate::domain::flow::Entry;
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::inbound::temporal::activities::FactoryActivities;

/// A review panel's visit: the entry, which visit of it, and the place of
/// its first Step.
pub(super) struct Panel<'a> {
    pub(super) run_id: RunId,
    pub(super) entry: &'a Entry,
    pub(super) visit: u16,
    pub(super) seq: u16,
}

/// What a review round came to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Round {
    /// Its outcome, for the Flow to route.
    Routed(Outcome),
    /// The Run ended: cancelled, or before the round's Steps started.
    Ended(RunOutcome),
}

/// What a review round asks of Temporal: its activities, and whether the
/// Run was cancelled.
pub(super) trait RoundWork {
    /// Why an activity failed for good.
    type Failure;

    /// Whether the Run's workflow was cancelled.
    fn cancelled(&self) -> bool;

    /// Open the round: whether it opened.
    async fn open(&self, opening: ReviewRoundOpening) -> Result<bool, Self::Failure>;

    /// Record that a reviewer's Step started.
    async fn start(&self, request: StepRequest) -> Result<StepStart, Self::Failure>;

    /// Run the reviewers' work at once, and return each one's result in
    /// the order of `requests`.
    async fn run_all(&self, requests: &[StepRequest]) -> Vec<Result<StepReport, Self::Failure>>;

    /// Record how a reviewer's Step ended.
    async fn end(&self, ending: StepEnding) -> Result<(), Self::Failure>;

    /// Let go of a reviewer's session once its end is recorded.
    async fn release(&self, request: StepRequest) -> Result<(), Self::Failure>;

    /// Stop a reviewer's agent for good.
    async fn stop(&self, request: &StepRequest);

    /// Close the round, and return its outcome.
    async fn close(&self, closing: ReviewRoundClosing) -> Result<Outcome, Self::Failure>;
}

/// Run `panel`'s review round: open it, start a Step for each reviewer, in
/// the order the entry names them, run their work at once, record each
/// Step's end in that order, whichever ended first, and close the round,
/// whose outcome comes back. `held` names the Steps whose agents may still
/// hold what their work used, for a cancelled Run to stop.
pub(super) async fn review_round<W: RoundWork>(
    work: &W,
    panel: &Panel<'_>,
    held: &mut Vec<StepRequest>,
) -> Result<Round, W::Failure> {
    let opening = ReviewRoundOpening {
        run_id: panel.run_id,
        seq: panel.seq,
        round: panel.visit,
        reviewers: u16::try_from(panel.entry.reviewers.len()).unwrap_or(u16::MAX),
    };
    match work.open(opening).await {
        Ok(true) => {}
        // A round that could not open recorded why, and runs no Step.
        Ok(false) => return Ok(Round::Routed(Outcome::failed())),
        Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
        Err(_) => return Ok(Round::Routed(Outcome::failed())),
    }

    let requests: Vec<StepRequest> = panel
        .entry
        .reviewers
        .iter()
        .zip(panel.seq..)
        .map(|(role, seq)| {
            let key = StepKey {
                run_id: panel.run_id,
                seq,
            };
            StepRequest::reviewer(key, panel.entry, panel.visit, role, panel.seq)
        })
        .collect();
    for request in &requests {
        match work.start(request.clone()).await {
            Ok(StepStart::Started) => {}
            Ok(StepStart::RunEnded(outcome)) => return Ok(Round::Ended(outcome)),
            Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
            Err(error) => return Err(error),
        }
    }

    held.clone_from(&requests);
    let worked = work.run_all(&requests).await;

    let mut any_failed = false;
    for (request, worked) in requests.iter().zip(worked) {
        let (report, failed) = match worked {
            Ok(report) => (report, false),
            Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
            Err(_) => (
                StepReport {
                    outcome: Outcome::failed(),
                    output: FAILED_STEP_OUTPUT.to_string(),
                },
                true,
            ),
        };
        let ending = StepEnding {
            key: request.key,
            report,
        };
        if let Err(error) = work.end(ending).await {
            if work.cancelled() {
                return Ok(Round::Ended(RunOutcome::Cancelled));
            }
            return Err(error);
        }

        // As for any Step: work that failed for good has its agent
        // stopped, and a Step that ended lets go of its session.
        held.retain(|kept| kept.key != request.key);
        if failed {
            any_failed = true;
            work.stop(request).await;
            continue;
        }
        if let Err(error) = work.release(request.clone()).await {
            if work.cancelled() {
                held.push(request.clone());
                return Ok(Round::Ended(RunOutcome::Cancelled));
            }
            return Err(error);
        }
    }

    let last = panel
        .entry
        .max_visits
        .is_some_and(|max| panel.visit >= u16::from(max.get()));
    let closing = ReviewRoundClosing {
        run_id: panel.run_id,
        seq: panel.seq,
        failed: any_failed,
        last,
    };
    match work.close(closing).await {
        Ok(outcome) => Ok(Round::Routed(outcome)),
        Err(_) if work.cancelled() => Ok(Round::Ended(RunOutcome::Cancelled)),
        Err(_) => Ok(Round::Routed(Outcome::failed())),
    }
}

/// A review round's activities on the workflow's context.
pub(super) struct TemporalRound<'a> {
    pub(super) ctx: &'a WorkflowContext<FlowWorkflow>,
    pub(super) work_options: &'a ActivityOptions,
    pub(super) record_options: &'a ActivityOptions,
}

impl RoundWork for TemporalRound<'_> {
    type Failure = ActivityExecutionError;

    fn cancelled(&self) -> bool {
        is_cancelled(self.ctx)
    }

    async fn open(&self, opening: ReviewRoundOpening) -> Result<bool, Self::Failure> {
        self.ctx
            .execute_activity(
                FactoryActivities::open_review_round,
                opening,
                ActivityOptions::start_to_close_timeout(OPEN_ROUND_TIMEOUT),
            )
            .await
    }

    async fn start(&self, request: StepRequest) -> Result<StepStart, Self::Failure> {
        self.ctx
            .execute_activity(
                FactoryActivities::start_step,
                request,
                self.record_options.clone(),
            )
            .await
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
    }

    async fn end(&self, ending: StepEnding) -> Result<(), Self::Failure> {
        self.ctx
            .execute_activity(
                FactoryActivities::end_step,
                ending,
                self.record_options.clone(),
            )
            .await
    }

    async fn release(&self, request: StepRequest) -> Result<(), Self::Failure> {
        self.ctx
            .execute_activity(
                FactoryActivities::release_step,
                request,
                self.record_options.clone(),
            )
            .await
    }

    async fn stop(&self, request: &StepRequest) {
        stop(self.ctx, request).await;
    }

    async fn close(&self, closing: ReviewRoundClosing) -> Result<Outcome, Self::Failure> {
        let options = ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::StartToClose(
            CLOSE_ROUND_TIMEOUT,
        ))
        .heartbeat_timeout(STEP_HEARTBEAT_TIMEOUT)
        .build();
        self.ctx
            .execute_activity(FactoryActivities::close_review_round, closing, options)
            .await
    }
}
