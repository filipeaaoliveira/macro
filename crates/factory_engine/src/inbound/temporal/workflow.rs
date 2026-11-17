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
//! An entry with `reviewers` is a review panel: the workflow opens a review
//! round, starts a Step for each reviewer, runs their work at once, records
//! their ends in the entry's order, and closes the round, whose outcome the
//! interpreter gets. Only such an entry takes that path, and a Run carries
//! its Flow in its input, so Runs started on a Flow without one replay as
//! before.
//!
//! A Step whose agent asks the Run's owner a question (only one whose Flow
//! says how many, `questions`) keeps its session; the workflow opens a
//! question Gate, waits for the answer, and has the same Role go on in a
//! Step of its own, at the next place, in that session. One question past
//! the Step's ends the Run `manual`. A Flow without `questions`, as every
//! Run started before them carries, never takes that path.
//!
//! Cancelling the workflow cancels the running Step, whose activity drops
//! its work before the cancellation completes, or the wait for an answer.
//! The workflow then stops the Step's agent, which only it can tell was
//! cancelled with the Run, and records the Run's end as `cancelled`, which
//! also ends its open Steps. A Step whose work failed for good has its agent
//! stopped too; one that ended has its agent let go once its end is
//! recorded.

mod review_round;
mod step;
mod work;

use std::time::Duration;

use factory_records::domain::models::{Outcome, RunOutcome, StepKey};
use temporalio_common::error::ApplicationFailure;
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{
    ActivityCancellationType, ActivityCloseTimeouts, ActivityOptions, SyncWorkflowContext,
    WorkflowCancellationToken, WorkflowContext, WorkflowContextView, WorkflowResult,
    WorkflowTermination,
};

use crate::domain::flow::Flow;
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening, OpenGate, check_answer};
use crate::domain::interpreter::{Interpreter, NextStep};
use crate::domain::models::{RunFinish, RunStart, StepReport, StepRequest};
use crate::domain::questions::gate_question;
use crate::inbound::temporal::activities::FactoryActivities;
use crate::temporal::{ANSWER_GATE_UPDATE, FLOW_WORKFLOW};
use review_round::{Panel, Round, review_round};
use step::walk_step;
use work::TemporalWork;

/// What a Step whose work failed for good shows its owner.
const FAILED_STEP_OUTPUT: &str = "The step's work failed. The factory service's logs say why.";

/// How long one attempt of a Step's work may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long a Step's work may go without a heartbeat before Temporal counts
/// its worker as gone and tries again elsewhere, in the same agent session.
/// How soon a cancel lands is the worker's heartbeat throttle's to set, not
/// this.
const STEP_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a Step's work may take in all, across its attempts. Attempts
/// are not counted: a worker shutting down fails the attempt it runs, so a
/// count would let a few deploys fail a healthy Step for good. Work that
/// would fail the same way again fails at once, and each attempt gives up on
/// its agent session within its own timeout.
const STEP_TOTAL: Duration = Duration::from_secs(2 * 60 * 60);

/// How long recording a Step, a Gate or a Run's end may take.
const RECORD_TIMEOUT: Duration = Duration::from_secs(60);

/// How long opening a review round may take: it reads GitHub.
const OPEN_ROUND_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long one attempt to close a review round may take: it reads GitHub
/// and makes a task of each follow-up, heartbeating as it goes.
const CLOSE_ROUND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How long closing a review round may take in all, across its attempts:
/// past it the round fails, and the Flow routes `failed`.
const CLOSE_ROUND_TOTAL: Duration = Duration::from_secs(2 * 60 * 60);

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
        // The Steps whose agents may still hold what their work used: from
        // a Step's work's start until its agent is stopped or let go, and
        // the Steps it goes on from, whose session it works in.
        let mut held: Vec<StepRequest> = Vec::new();
        // Whether agents may ask the Run's owner questions, and how many
        // each Step's work may ask. A Flow without them, as every Run
        // started before them carries, never takes their path, so its
        // history replays as before.
        let questions = Questions::of(&start.flow);
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
            let temporal = TemporalWork {
                ctx,
                work_options: &work_options,
                record_options: &record_options,
            };
            let mut resume = || interpreter.resume(&mut state).ok().flatten();

            // A review panel runs its reviewers' Steps at once, as one
            // review round, whose outcome the Flow routes; any other entry
            // runs one Step, and the Steps that go on from its agent's
            // questions.
            let walked = if entry.reviewers.is_empty() {
                let key = StepKey {
                    run_id: start.run_id,
                    seq,
                };
                let request = questions.offer(StepRequest::of(key, entry, visit), 0);
                walk_step(&temporal, entry, request, questions, &mut held, &mut resume).await?
            } else {
                let panel = Panel {
                    run_id: start.run_id,
                    entry,
                    visit,
                    seq,
                    questions,
                };
                review_round(&temporal, &panel, &mut held, &mut resume).await?
            };
            match walked {
                Round::Routed(outcome) => {
                    if let Err(error) = interpreter.record(&mut state, &outcome) {
                        stopped = Some(error);
                        break RunOutcome::Failed;
                    }
                }
                Round::Ended(outcome) => break outcome,
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

        // A cancelled Run's Step may still have its agent at work, and a
        // Run that ended at an agent's question still holds its session.
        // The Run reads as ended first: stopping an agent may take a while.
        // Any other end holds no Step.
        for request in &held {
            stop(ctx, request).await;
        }

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
async fn stop(ctx: &WorkflowContext<FlowWorkflow>, request: &StepRequest) {
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

/// Open the Gate `opening`, wait for a person's answer, record it, and
/// return it.
async fn ask(
    ctx: &WorkflowContext<FlowWorkflow>,
    opening: GateOpening,
) -> WorkflowResult<AnswerGate> {
    let record_options = ActivityOptions::start_to_close_timeout(RECORD_TIMEOUT);
    let key = opening.key;
    let answers = opening.answers.clone();
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
            answers,
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

    ctx.execute_activity(
        FactoryActivities::answer_gate,
        GateAnswered {
            key,
            answer: answer.clone(),
        },
        record_options,
    )
    .await?;
    Ok(answer)
}

/// Whether a Run's agents may ask its owner questions, and how many each
/// Step's work may ask, as its Flow says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Questions {
    /// Whether agents may ask.
    pub(super) may_ask: bool,
    /// How many questions each Step's work may ask; the next one ends the
    /// Run for a person.
    pub(super) most: u8,
}

impl Questions {
    /// The questions `flow` lets its agents ask: none when it sets none,
    /// or zero.
    pub(super) fn of(flow: &Flow) -> Self {
        let most = flow.questions.unwrap_or(0);
        Self {
            may_ask: most > 0,
            most,
        }
    }

    /// `request`, whose work asked `asked` questions so far, offering its
    /// agent the questions left, when agents may ask.
    pub(super) fn offer(self, request: StepRequest, asked: u8) -> StepRequest {
        if !self.may_ask {
            return request;
        }
        StepRequest {
            may_ask: true,
            questions_left: self.most.saturating_sub(asked),
            ..request
        }
    }

    /// The question the Step `request` ended with, `report`, when it asked
    /// one: only a Step that may ask does.
    pub(super) fn asked(self, request: &StepRequest, report: &StepReport) -> Option<Question> {
        (self.may_ask && request.may_ask && report.outcome == Outcome::question()).then(|| {
            Question {
                text: gate_question(&report.output),
            }
        })
    }
}

/// A question a Step's agent asked the Run's owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Question {
    /// The question, as its Gate keeps it.
    text: String,
}

impl Question {
    /// The question Gate of the Step `key` that asks it, whose one answer
    /// carries the owner's words in its note.
    pub(super) fn opening(self, key: StepKey) -> GateOpening {
        GateOpening {
            key,
            question: self.text,
            answers: vec![Outcome::answer()],
        }
    }
}
