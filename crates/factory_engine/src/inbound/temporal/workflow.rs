//! `FlowWorkflow`: the one workflow that walks any Flow.
//!
//! It asks the interpreter for the next Step, records its start, runs its
//! work, records its end, and hands the interpreter the Step's outcome, until
//! the Run ends; then it records the end. Each record is its own activity,
//! so a failed write is retried without repeating the Step's work. An entry
//! with a Gate waits, once its Step ends `done`, for a person's answer,
//! delivered as an update; the answer is the outcome the interpreter gets.
//! The interpreter is pure, so a replay walks the Flow the same way from the
//! recorded outcomes and answers.
//!
//! Cancelling the workflow cancels the running Step, whose activity drops
//! its work before the cancellation completes, or the wait for an answer.
//! The workflow then stops the Step's agent, which only it can tell was
//! cancelled with the Run, and records the Run's end as `cancelled`, which
//! also ends its open Steps. A Step whose work failed for good has its agent
//! stopped too; one that ended has its agent let go once its end is
//! recorded.

use std::time::Duration;

use factory_records::domain::models::{Outcome, RunOutcome, StepKey};
use temporalio_common::error::ApplicationFailure;
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{
    ActivityCancellationType, ActivityCloseTimeouts, ActivityOptions, SyncWorkflowContext,
    WorkflowCancellationToken, WorkflowContext, WorkflowContextView, WorkflowResult,
    WorkflowTermination,
};

use crate::domain::flow::GateDefinition;
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening, OpenGate, check_answer};
use crate::domain::interpreter::{Interpreter, NextStep};
use crate::domain::models::{RunFinish, RunStart, StepEnding, StepReport, StepRequest, StepStart};
use crate::inbound::temporal::activities::FactoryActivities;
use crate::temporal::{ANSWER_GATE_UPDATE, FLOW_WORKFLOW};

/// What a Step whose work failed for good shows its owner.
const FAILED_STEP_OUTPUT: &str = "The step's work failed. The factory service's logs say why.";

/// How long one attempt of a Step's work may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long a Step's work may go without a heartbeat before Temporal counts
/// its worker as gone and tries again elsewhere.
const STEP_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a Step's work may take in all, across its attempts. Attempts
/// are not counted: a worker shutting down fails the attempt it runs, so a
/// count would let a few deploys fail a healthy Step for good. Work that
/// would fail the same way again fails at once, and each attempt gives up on
/// its agent session within its own timeout.
const STEP_TOTAL: Duration = Duration::from_secs(2 * 60 * 60);

/// How long recording a Step, a Gate or a Run's end may take.
const RECORD_TIMEOUT: Duration = Duration::from_secs(60);

/// How long one attempt to stop a Step's agent may take: past the agent's
/// own watch of its stopped session, and the session's delete.
const STOP_TIMEOUT: Duration = Duration::from_secs(3 * 60);

/// How long stopping a Step's agent may take in all, across its attempts,
/// before it is given up on.
const STOP_TOTAL: Duration = Duration::from_secs(15 * 60);

/// The workflow of one Run.
#[workflow]
#[derive(Default)]
pub struct FlowWorkflow {
    /// The Gate the Run waits on, if any.
    open: Option<OpenGate>,
    /// The answer to it, once a person gave one the Gate takes.
    answer: Option<AnswerGate>,
}

#[workflow_methods]
impl FlowWorkflow {
    /// Walk `start`'s Flow to its end, and return how the Run ended.
    #[run(name = FLOW_WORKFLOW)]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        start: RunStart,
    ) -> WorkflowResult<RunOutcome> {
        let interpreter = Interpreter::new(&start.flow);
        let mut state = interpreter.start();
        // The Step's work is dropped before its cancellation completes, so
        // the agent is stopped after it, and the Run's `cancelled` end
        // closes the Step, so a late end of it records nothing.
        let work_options =
            ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::ScheduleAndStartToClose {
                schedule_to_close: STEP_TOTAL,
                start_to_close: STEP_TIMEOUT,
            })
            .heartbeat_timeout(STEP_HEARTBEAT_TIMEOUT)
            .cancellation_type(ActivityCancellationType::WaitCancellationCompleted)
            .build();
        let record_options = ActivityOptions::start_to_close_timeout(RECORD_TIMEOUT);

        // A Flow that cannot go on still records the Run's end, then fails
        // the workflow with why, for Temporal to show.
        let mut stopped = None;
        // The Step whose agent may still hold what its work used: from its
        // work's start until the agent is stopped or let go.
        let mut held = None;
        let outcome = loop {
            let next = match interpreter.next_step(&mut state) {
                Ok(next) => next,
                Err(error) => {
                    stopped = Some(error);
                    break RunOutcome::Failed;
                }
            };
            let (entry, visit, seq) = match next {
                NextStep::End(outcome) => break outcome,
                NextStep::Run { entry, visit, seq } => (entry, visit, seq),
            };

            let key = StepKey {
                run_id: start.run_id,
                seq,
            };
            let request = StepRequest {
                key,
                entry: entry.id.clone(),
                step: entry.step.clone(),
                visit,
            };
            let started = ctx
                .execute_activity(
                    FactoryActivities::start_step,
                    request.clone(),
                    record_options.clone(),
                )
                .await;
            match started {
                Ok(StepStart::Started) => {}
                // The Run was cancelled before this workflow started: it
                // ends as the Run did, never as a failure.
                Ok(StepStart::RunEnded) => break RunOutcome::Cancelled,
                Err(_) if is_cancelled(ctx) => break RunOutcome::Cancelled,
                Err(error) => return Err(error.into()),
            }

            // A Step that still fails for good ends `failed`, with
            // why as its output; the Flow may route it like any other
            // outcome. A Step stopped by cancelling the Run ends the Run.
            held = Some(request.clone());
            let worked = ctx
                .execute_activity(
                    FactoryActivities::run_step,
                    request.clone(),
                    work_options.clone(),
                )
                .await;
            let report = match worked {
                Ok(report) => report,
                Err(_) if is_cancelled(ctx) => break RunOutcome::Cancelled,
                Err(_) => {
                    // The work failed for good: stop its agent, so an entry
                    // that fixes or retries it never runs beside it.
                    stop(ctx, &request).await;
                    held = None;
                    // The owner reads the output; why the work failed is
                    // internal detail, which the worker logged.
                    StepReport {
                        outcome: Outcome::failed(),
                        output: FAILED_STEP_OUTPUT.to_string(),
                    }
                }
            };
            let reported = report.outcome.clone();
            let ended = ctx
                .execute_activity(
                    FactoryActivities::end_step,
                    StepEnding { key, report },
                    record_options.clone(),
                )
                .await;
            if ended.is_err() {
                if is_cancelled(ctx) {
                    break RunOutcome::Cancelled;
                }
                ended?;
            }

            // No attempt of the Step's work comes once its end is recorded,
            // so its agent lets go of the Step's session.
            if held.take().is_some() {
                let released = ctx
                    .execute_activity(
                        FactoryActivities::release_step,
                        request.clone(),
                        record_options.clone(),
                    )
                    .await;
                if released.is_err() {
                    if is_cancelled(ctx) {
                        held = Some(request.clone());
                        break RunOutcome::Cancelled;
                    }
                    released?;
                }
            }

            let outcome = match &entry.gate {
                Some(gate) if reported == Outcome::done() => match ask(ctx, key, gate).await {
                    Ok(answer) => answer,
                    Err(_) if is_cancelled(ctx) => break RunOutcome::Cancelled,
                    Err(termination) => return Err(termination),
                },
                _ => reported,
            };
            if let Err(error) = interpreter.record(&mut state, &outcome) {
                stopped = Some(error);
                break RunOutcome::Failed;
            }
        };

        // A cancelled Run's Step may still have its agent at work.
        if outcome == RunOutcome::Cancelled
            && let Some(request) = held
        {
            stop(ctx, &request).await;
        }

        // A cancelled workflow records its end on a token of its own, which
        // the cancellation does not reach.
        let finish = RunFinish {
            run_id: start.run_id,
            outcome,
        };
        ctx.execute_activity(
            FactoryActivities::end_run,
            finish,
            ActivityOptions::with_start_to_close_timeout(RECORD_TIMEOUT)
                .cancellation_token(WorkflowCancellationToken::new())
                .build(),
        )
        .await?;

        match (outcome, stopped) {
            (RunOutcome::Cancelled, _) => Err(WorkflowTermination::cancelled()),
            (_, Some(error)) => Err(ApplicationFailure::non_retryable(error).into()),
            (_, None) => Ok(outcome),
        }
    }

    /// Give a person's answer to the Gate the Run waits on.
    #[update(name = ANSWER_GATE_UPDATE)]
    pub fn answer_gate(&mut self, _ctx: &mut SyncWorkflowContext<Self>, answer: AnswerGate) {
        self.answer = Some(answer);
    }

    /// Refuse an answer to a Gate the Run does not wait on, a second answer,
    /// and an answer the Gate does not offer.
    #[update_validator(answer_gate)]
    fn check_answer_gate(
        &self,
        _ctx: &WorkflowContextView,
        answer: &AnswerGate,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        check_answer(self.open.as_ref(), self.answer.is_some(), answer).map_err(Into::into)
    }
}

/// Stop the agent of the Step `request`, for good: after the Run was
/// cancelled, on a token of its own the cancellation does not reach, or after
/// its work failed for good. It is tried until [`STOP_TOTAL`], then given up
/// on: the Run must not wait on an agent that won't answer.
async fn stop(ctx: &mut WorkflowContext<FlowWorkflow>, request: &StepRequest) {
    let options =
        ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::ScheduleAndStartToClose {
            schedule_to_close: STOP_TOTAL,
            start_to_close: STOP_TIMEOUT,
        })
        .heartbeat_timeout(STEP_HEARTBEAT_TIMEOUT)
        .cancellation_token(WorkflowCancellationToken::new())
        .build();
    // A stop given up on shows in the workflow's history; the worker logged
    // why.
    let _ = ctx
        .execute_activity(FactoryActivities::stop_step, request.clone(), options)
        .await;
}

/// Whether the Run's workflow was cancelled.
fn is_cancelled(ctx: &WorkflowContext<FlowWorkflow>) -> bool {
    ctx.cancellation_token().is_cancelled()
}

/// Open the Gate of the Step `key`, wait for a person's answer, record it,
/// and return it as the entry's outcome.
async fn ask(
    ctx: &mut WorkflowContext<FlowWorkflow>,
    key: StepKey,
    gate: &GateDefinition,
) -> WorkflowResult<Outcome> {
    let record_options = ActivityOptions::start_to_close_timeout(RECORD_TIMEOUT);
    let opening = GateOpening {
        key,
        question: gate.question.clone(),
        answers: gate.answers.clone(),
    };
    ctx.execute_activity(
        FactoryActivities::open_gate,
        opening,
        record_options.clone(),
    )
    .await?;

    // Answers are taken only once the Gate is recorded, so a person never
    // answers a Gate the Run's records do not show.
    ctx.state_mut(|workflow| {
        workflow.open = Some(OpenGate {
            seq: key.seq,
            answers: gate.answers.clone(),
        });
        workflow.answer = None;
    });

    // The Gate closes in the same change that takes its answer, so no
    // second answer slips in between.
    let answer = loop {
        ctx.wait_condition(|workflow| workflow.answer.is_some())
            .await?;
        let taken = ctx.state_mut(|workflow| {
            let answer = workflow.answer.take();
            if answer.is_some() {
                workflow.open = None;
            }
            answer
        });
        if let Some(answer) = taken {
            break answer;
        }
    };

    let outcome = answer.answer.clone();
    ctx.execute_activity(
        FactoryActivities::answer_gate,
        GateAnswered { key, answer },
        record_options,
    )
    .await?;
    Ok(outcome)
}
