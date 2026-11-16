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
//! its work and asks the Step's agent to stop before the cancellation
//! completes, or the wait for an answer; the Run's end is then recorded as
//! `cancelled`, which also ends its open Steps.

use std::time::Duration;

use factory_records::domain::models::{MAX_OUTPUT_BYTES, Outcome, RunOutcome, StepKey};
use temporalio_common::RetryPolicy;
use temporalio_common::error::ApplicationFailure;
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{
    ActivityCancellationType, ActivityOptions, SyncWorkflowContext, WorkflowCancellationToken,
    WorkflowContext, WorkflowContextView, WorkflowResult, WorkflowTermination,
};

use crate::domain::flow::GateDefinition;
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening, OpenGate, check_answer};
use crate::domain::interpreter::{Interpreter, NextStep};
use crate::domain::models::{RunFinish, RunStart, StepEnding, StepReport, StepRequest};
use crate::inbound::temporal::activities::FactoryActivities;
use crate::temporal::{ANSWER_GATE_UPDATE, FLOW_WORKFLOW};

/// How long one attempt of a Step's work may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long a Step's work may go without a heartbeat before Temporal counts
/// its worker as gone and tries again elsewhere.
const STEP_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(60);

/// How many times a Step's work is tried before it ends `failed`.
const STEP_ATTEMPTS: u32 = 3;

/// How long recording a Step, a Gate or a Run's end may take.
const RECORD_TIMEOUT: Duration = Duration::from_secs(60);

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
        // The Step's work is dropped and its agent asked to stop before its
        // cancellation completes, and the Run's `cancelled` end closes the
        // Step, so a late end of it records nothing.
        let work_options = ActivityOptions::with_start_to_close_timeout(STEP_TIMEOUT)
            .heartbeat_timeout(STEP_HEARTBEAT_TIMEOUT)
            .cancellation_type(ActivityCancellationType::WaitCancellationCompleted)
            .retry_policy(
                RetryPolicy::builder()
                    .maximum_attempts(STEP_ATTEMPTS)
                    .build(),
            )
            .build();
        let record_options = ActivityOptions::start_to_close_timeout(RECORD_TIMEOUT);
        // Stopping a failed Step's agent is tried a few times, then given up
        // on: the Step's end must not wait on an agent that won't answer.
        let stop_options = ActivityOptions::with_start_to_close_timeout(RECORD_TIMEOUT)
            .retry_policy(
                RetryPolicy::builder()
                    .maximum_attempts(STEP_ATTEMPTS)
                    .build(),
            )
            .build();

        // A Flow that cannot go on still records the Run's end, then fails
        // the workflow with why, for Temporal to show.
        let mut stopped = None;
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
            if started.is_err() {
                if is_cancelled(ctx) {
                    break RunOutcome::Cancelled;
                }
                started?;
            }

            // A Step that still fails after its retries ends `failed`, with
            // why as its output; the Flow may route it like any other
            // outcome. A Step stopped by cancelling the Run ends the Run.
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
                Err(error) => {
                    // The work failed for good: stop its agent, so an entry
                    // that fixes or retries it never runs beside it.
                    let stopped = ctx
                        .execute_activity(
                            FactoryActivities::stop_step,
                            request.clone(),
                            stop_options.clone(),
                        )
                        .await;
                    if stopped.is_err() && is_cancelled(ctx) {
                        break RunOutcome::Cancelled;
                    }
                    StepReport {
                        outcome: Outcome::failed(),
                        output: bounded(format!("The step failed: {error}")),
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

/// Whether the Run's workflow was cancelled.
fn is_cancelled(ctx: &WorkflowContext<FlowWorkflow>) -> bool {
    ctx.cancellation_token().is_cancelled()
}

/// `text`, cut to what a Step's output keeps, on a character boundary.
fn bounded(mut text: String) -> String {
    if text.len() > MAX_OUTPUT_BYTES {
        let mut end = MAX_OUTPUT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
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
