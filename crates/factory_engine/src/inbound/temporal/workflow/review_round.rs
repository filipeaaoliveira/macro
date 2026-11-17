//! A review panel's visit: one review round. The round's walk is written
//! over [`RoundWork`], what it asks of Temporal, so the workflow runs it on
//! its context ([`TemporalRound`]) and the tests on a stand-in, without a
//! Temporal server. The walk issues the same activities in the same order
//! either way, so a replay walks it the same way.
//!
//! A reviewer whose agent asks the Run's owner a question ends its Step
//! `question`, and keeps its session; once every reviewer's Step ended,
//! each question goes to the owner on a question Gate, one at a time in the
//! entry's order, and the reviewer goes on with the answer in a Step of its
//! own, in that session. The round closes once every reviewer answered, on
//! what they all found: the others' results are kept meanwhile.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, RunId, RunOutcome, StepKey};
use temporalio_sdk::workflows::join_all;
use temporalio_sdk::{
    ActivityCloseTimeouts, ActivityOptions, WorkflowContext, WorkflowTermination,
};

use super::{
    CLOSE_ROUND_TIMEOUT, CLOSE_ROUND_TOTAL, FAILED_STEP_OUTPUT, FlowWorkflow, OPEN_ROUND_TIMEOUT,
    Question, Questions, STEP_HEARTBEAT_TIMEOUT, ask, is_cancelled, stop,
};
use crate::domain::flow::Entry;
use crate::domain::gate::GateOpening;
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
    /// Whether its reviewers may ask the Run's owner questions, and how
    /// many each.
    pub(super) questions: Questions,
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

    /// Put a reviewer's question to the Run's owner on the Gate `opening`,
    /// and wait for the answer.
    async fn ask(&self, opening: GateOpening) -> Result<(), Self::Failure>;

    /// Close the round, and return its outcome.
    async fn close(&self, closing: ReviewRoundClosing) -> Result<Outcome, Self::Failure>;
}

/// Run `panel`'s review round: open it, start a Step for each reviewer, in
/// the order the entry names them, run their work at once, record each
/// Step's end in that order, whichever ended first, put each reviewer's
/// question to the Run's owner and have it go on with the answer, and close
/// the round, whose outcome comes back. `held` names the Steps whose agents
/// may still hold what their work used, for the workflow to stop when the
/// Run ends. `resume` gives the place of a Step that goes on from a
/// question, none once the Run took its most Steps.
pub(super) async fn review_round<W: RoundWork>(
    work: &W,
    panel: &Panel<'_>,
    held: &mut Vec<StepRequest>,
    resume: &mut impl FnMut() -> Option<u16>,
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
                .asking(panel.questions.may_ask)
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
    let mut asking: Vec<(StepRequest, Question)> = Vec::new();
    for (request, worked) in requests.iter().zip(worked) {
        match end_reviewer(work, panel.questions, request, worked, held).await? {
            Reviewed::Ended(Ended::Done) => {}
            Reviewed::Ended(Ended::Failed) => any_failed = true,
            Reviewed::Asked(question) => asking.push((request.clone(), question)),
            Reviewed::RunEnded(outcome) => return Ok(Round::Ended(outcome)),
        }
    }

    // Each question waits for the owner's answer, and its reviewer goes on
    // with it, until it hands back its review, fails, or asks more than
    // its Step's questions: then the round fails, and the Run ends for a
    // person.
    for (asking, question) in asking {
        let (mut asking, mut question) = (asking, question);
        let mut asked: u8 = 0;
        loop {
            asked = asked.saturating_add(1);
            if asked > panel.questions.most {
                return stop_round(work, panel, RunOutcome::Manual).await;
            }
            match work.ask(question.opening(asking.key)).await {
                Ok(()) => {}
                Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
                Err(error) => return Err(error),
            }
            let Some(seq) = resume() else {
                return stop_round(work, panel, RunOutcome::Blocked).await;
            };
            let going_on = asking.going_on(StepKey {
                run_id: panel.run_id,
                seq,
            });
            match work.start(going_on.clone()).await {
                Ok(StepStart::Started) => {}
                Ok(StepStart::RunEnded(outcome)) => return Ok(Round::Ended(outcome)),
                Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
                Err(error) => return Err(error),
            }
            // The Step that goes on holds the asking Step's session now.
            held.retain(|kept| kept.key != asking.key);
            held.push(going_on.clone());
            let worked = work
                .run_all(std::slice::from_ref(&going_on))
                .await
                .into_iter()
                .next();
            let Some(worked) = worked else {
                return Ok(Round::Ended(RunOutcome::Failed));
            };
            match end_reviewer(work, panel.questions, &going_on, worked, held).await? {
                Reviewed::Ended(Ended::Done) => break,
                Reviewed::Ended(Ended::Failed) => {
                    any_failed = true;
                    break;
                }
                Reviewed::Asked(again) => {
                    asking = going_on;
                    question = again;
                }
                Reviewed::RunEnded(outcome) => return Ok(Round::Ended(outcome)),
            }
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

/// How a reviewer's Step ended, once recorded.
enum Reviewed {
    /// It ended, and let go of its session or had its agent stopped.
    Ended(Ended),
    /// Its agent asked the Run's owner this question, and keeps its
    /// session for the Step that goes on with the answer.
    Asked(Question),
    /// The Run ended meanwhile.
    RunEnded(RunOutcome),
}

/// How a reviewer's Step that did not ask ended.
enum Ended {
    /// With its review.
    Done,
    /// Its work failed for good.
    Failed,
}

/// Record how the reviewer's Step `request` ended, from its work
/// `worked`: as for any Step, work that failed for good has its agent
/// stopped, and a Step that ended lets go of its session, unless it asked
/// the Run's owner a question.
async fn end_reviewer<W: RoundWork>(
    work: &W,
    questions: Questions,
    request: &StepRequest,
    worked: Result<StepReport, W::Failure>,
    held: &mut Vec<StepRequest>,
) -> Result<Reviewed, W::Failure> {
    let (report, failed) = match worked {
        Ok(report) => (report, false),
        Err(_) if work.cancelled() => return Ok(Reviewed::RunEnded(RunOutcome::Cancelled)),
        Err(_) => (
            StepReport {
                outcome: Outcome::failed(),
                output: FAILED_STEP_OUTPUT.to_string(),
            },
            true,
        ),
    };
    let question = questions.asked(request, &report);
    let ending = StepEnding {
        key: request.key,
        report,
    };
    if let Err(error) = work.end(ending).await {
        if work.cancelled() {
            return Ok(Reviewed::RunEnded(RunOutcome::Cancelled));
        }
        return Err(error);
    }
    if let Some(question) = question {
        return Ok(Reviewed::Asked(question));
    }

    held.retain(|kept| kept.key != request.key);
    if failed {
        work.stop(request).await;
        return Ok(Reviewed::Ended(Ended::Failed));
    }
    if let Err(error) = work.release(request.clone()).await {
        if work.cancelled() {
            held.push(request.clone());
            return Ok(Reviewed::RunEnded(RunOutcome::Cancelled));
        }
        return Err(error);
    }
    Ok(Reviewed::Ended(Ended::Done))
}

/// Close `panel`'s round as failed, as a reviewer's question stopped it,
/// and end the Run `outcome`: the reviewers still held are stopped once the
/// Run's end is recorded.
async fn stop_round<W: RoundWork>(
    work: &W,
    panel: &Panel<'_>,
    outcome: RunOutcome,
) -> Result<Round, W::Failure> {
    let closing = ReviewRoundClosing {
        run_id: panel.run_id,
        seq: panel.seq,
        failed: true,
        last: false,
    };
    match work.close(closing).await {
        Err(_) if work.cancelled() => Ok(Round::Ended(RunOutcome::Cancelled)),
        _ => Ok(Round::Ended(outcome)),
    }
}

/// A review round's activities on the workflow's context.
pub(super) struct TemporalRound<'a> {
    pub(super) ctx: &'a WorkflowContext<FlowWorkflow>,
    pub(super) work_options: &'a ActivityOptions,
    pub(super) record_options: &'a ActivityOptions,
}

impl RoundWork for TemporalRound<'_> {
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

    async fn ask(&self, opening: GateOpening) -> Result<(), Self::Failure> {
        ask(self.ctx, opening).await.map(|_| ())
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
