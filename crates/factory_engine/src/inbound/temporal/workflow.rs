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
//! Once a Flow's Gate is recorded, and before a person may answer it, the
//! worker asks the Run's Standing Rules about it (`factory_rule_gate`): a
//! Rule Answer comes back as the activity's result, so a replay reads it
//! from the history and walks the same way. That activity is behind the
//! [`STANDING_RULES_PATCH`] marker: a Run whose history opened a Gate
//! before it replays without asking, and a Gate a Run opens on a newer
//! worker asks. A review panel's Gate (`after` a round's outcome) asks once
//! the round ends with that outcome; only a Flow that has one takes that
//! path, so Runs on a Flow without one replay as before. A question Gate
//! is never put to the rules: only the Run's owner answers an agent.
//!
//! A Gate left for a person, a Flow's Gate neither a rule nor the stand-in
//! answered (or whose stand-in answer was taken back) or an agent's
//! question, is told to the Run's owner once the workflow takes its
//! answers (`factory_notify_gate`, behind the [`GATE_OWNER_PATCH`]
//! marker); a Gate the stand-in answers tells no one. The worker notifies
//! only for a Run whose owner section is on, and its result changes
//! nothing the workflow decides. A Run whose
//! history waited on a Gate before the marker replays without it.
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
use temporalio_sdk::workflows::select;
use temporalio_sdk::{
    ActivityCancellationType, ActivityCloseTimeouts, ActivityOptions, CancellableFuture,
    SyncWorkflowContext, WorkflowCancellationToken, WorkflowContext, WorkflowContextView,
    WorkflowResult, WorkflowTermination,
};

use factory_records::domain::standing_rules::RuleGateKind;

use crate::domain::flow::Flow;
use crate::domain::gate::{
    AnswerGate, GateAnswered, GateOpening, GateRuling, OpenGate, ReplyGate, ReplyRead,
    ReplyReading, StandInAsking, StandInKeeping, check_answer, check_reply,
};
use crate::domain::interpreter::{Interpreter, NextStep};
use crate::domain::models::{RunFinish, RunStart, StepReport, StepRequest};
use crate::domain::questions::gate_question;
use crate::inbound::temporal::activities::FactoryActivities;
use crate::inbound::temporal::gate_activities::GateActivities;
use crate::temporal::{ANSWER_GATE_UPDATE, FLOW_WORKFLOW, REPLY_GATE_UPDATE};
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

/// How long telling a Run's owner of a Gate may take in all, across its
/// attempts, before the Gate waits without it.
const NOTIFY_TOTAL: Duration = Duration::from_secs(5 * 60);

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

/// The marker of the workflow that asks a Run's Standing Rules about each
/// Gate it opens. Renaming it breaks the replay of Runs that passed it.
pub(crate) const STANDING_RULES_PATCH: &str = "factory-standing-rules";

/// The marker of the workflow that tells the Run's owner of each Gate it
/// waits on for a person. Renaming it breaks the replay of Runs that
/// passed it.
pub(crate) const GATE_OWNER_PATCH: &str = "factory-gate-owner";

/// The marker of the workflow that asks the stand-in about each Flow's
/// Gate no rule answered, and takes a person's words as a reply. Renaming
/// it breaks the replay of Runs that passed it.
pub(crate) const GATE_REPLIES_PATCH: &str = "factory-gate-replies";

/// The workflow of one Run.
#[workflow]
#[derive(Default)]
pub struct FlowWorkflow {
    /// The Gate the Run waits on, if any.
    open: Option<OpenGate>,
    /// The answer to it, once a person gave one the Gate takes.
    answer: Option<AnswerGate>,
    /// A person's words on it, once they gave some the Gate takes.
    reply: Option<ReplyGate>,
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
        let taken = self.answer.is_some() || self.reply.is_some();
        check_answer(self.open.as_ref(), taken, answer).map_err(Into::into)
    }

    /// Give a person's words to the Gate the Run waits on, for the worker
    /// to read as one of its answers.
    #[update(name = REPLY_GATE_UPDATE)]
    pub fn reply_gate(&mut self, _ctx: &mut SyncWorkflowContext<Self>, reply: ReplyGate) {
        self.reply = Some(reply);
    }

    /// Refuse words on a Gate the Run does not wait on, words after an
    /// answer or other words it has not read yet, and words on a Gate
    /// opened before replies.
    #[update_validator(reply_gate)]
    fn check_reply_gate(
        &self,
        _ctx: &WorkflowContextView,
        reply: &ReplyGate,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let taken = self.answer.is_some() || self.reply.is_some();
        check_reply(self.open.as_ref(), taken, reply).map_err(Into::into)
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

/// What a Flow's Gate's Standing Rule reads of where the Run is. A
/// question Gate has none: no rule answers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Asking {
    /// The Gate's kind, from its Flow; none for a Gate no rule decides.
    pub(super) kind: Option<RuleGateKind>,
    /// Whether its Flow keeps it for a person only: the stand-in never
    /// answers it.
    pub(super) person_only: bool,
    /// For a review panel's Gate, the fix rounds in a row its Blockers
    /// went to.
    pub(super) fix_rounds: u16,
    /// For a review panel's Gate, the fix rounds in a row its rule may
    /// send.
    pub(super) fix_limit: u16,
}

/// Open the Gate `opening`; for a Flow's Gate (`rule`), ask the Run's
/// Standing Rules about it and return their Rule Answer, then ask the
/// stand-in; otherwise, or when neither answers, wait for a person's
/// answer, record it, and return it. Either way the answer is the outcome
/// the Run goes on with.
async fn ask(
    ctx: &WorkflowContext<FlowWorkflow>,
    opening: GateOpening,
    rule: Option<Asking>,
) -> WorkflowResult<Outcome> {
    let record_options = ActivityOptions::start_to_close_timeout(RECORD_TIMEOUT);
    let key = opening.key;
    let answers = opening.answers.clone();
    ctx.execute_activity(
        FactoryActivities::open_gate,
        opening,
        record_options.clone(),
    )
    .await?;

    // The rules are asked before a person may answer, so the two never
    // race: the worker records a Rule Answer itself. A Run whose history
    // opened this Gate before the marker existed replays without asking.
    // A question Gate never reaches the marker or the rules.
    if let Some(asking) = rule
        && ctx.patched(STANDING_RULES_PATCH)
        && let Some(kind) = asking.kind
    {
        let ruling = GateRuling {
            key,
            kind,
            fix_rounds: asking.fix_rounds,
            fix_limit: asking.fix_limit,
            answers: answers.clone(),
        };
        let ruled = ctx
            .execute_activity(FactoryActivities::rule_gate, ruling, record_options.clone())
            .await?;
        if let Some(ruled) = ruled {
            return Ok(ruled.answer);
        }
    }

    // A Gate opened before the marker takes only a person's answer, as
    // before: no stand-in and no reply. Its history replays without them.
    let replies = ctx.patched(GATE_REPLIES_PATCH);
    let gate = Waiting {
        key,
        question: rule.is_none(),
        answers,
        replies,
    };

    // The stand-in answers only a Flow's Gate of a kind, once no rule did.
    // Its answer stands once the owner let the window pass; an answer or a
    // reply of theirs in the window goes as any other.
    let mut overridden = None;
    if replies
        && let Some(asking) = rule
        && let Some(kind) = asking.kind
    {
        let standing = ctx
            .execute_activity(
                GateActivities::stand_in_gate,
                StandInAsking {
                    key,
                    kind,
                    person_only: asking.person_only,
                    fix_rounds: asking.fix_rounds,
                    fix_limit: asking.fix_limit,
                    answers: gate.answers.clone(),
                },
                record_options.clone(),
            )
            .await?;
        if let Some(standing) = standing {
            gate.open(ctx);
            let mut window = ctx.timer(Duration::from_secs(u64::from(standing.window_secs)));
            let woke = select! {
                waited = ctx.wait_condition(|workflow| workflow.answer.is_some() || workflow.reply.is_some()) => {
                    waited?;
                    true
                }
                _ = window => false,
            };
            if woke {
                window.cancel();
                overridden = gate.take(ctx);
            } else {
                // Closed first, so no answer slips in once the stand-in's
                // is the one the Run goes on with. A Run that now forbids
                // the stand-in (Tampered, under a Main Hold) gives the Gate
                // back to the person.
                gate.close(ctx);
                let keeping = StandInKeeping {
                    key,
                    kind,
                    person_only: asking.person_only,
                    fix_rounds: asking.fix_rounds,
                    fix_limit: asking.fix_limit,
                };
                let kept = ctx
                    .execute_activity(
                        GateActivities::keep_stand_in,
                        keeping,
                        record_options.clone(),
                    )
                    .await?;
                if let Some(kept) = kept {
                    return Ok(kept);
                }
            }
        }
    }

    // Answers are taken only once the Gate is recorded, so a person never
    // answers a Gate the Run's records do not show. An owner who overrode
    // the stand-in already gave theirs.
    if overridden.is_none() {
        gate.open(ctx);
    }
    let mut told = false;
    loop {
        let taken = if let Some(taken) = overridden.take() {
            Some(taken)
        } else {
            // The Gate now waits for a person: no rule and no stand-in
            // answered it, the stand-in's answer was taken back, or the
            // owner's words in its window were asked back. Its owner is
            // told once, while it takes answers, so the one they give from
            // the notification is taken.
            if !told {
                told = true;
                tell_owner(ctx, key).await;
            }
            if replies {
                ctx.wait_condition(|workflow| {
                    workflow.answer.is_some() || workflow.reply.is_some()
                })
                .await?;
            } else {
                ctx.wait_condition(|workflow| workflow.answer.is_some())
                    .await?;
            }
            gate.take(ctx)
        };

        match taken {
            None => {}
            Some(Taken::Answer(answer)) => {
                let outcome = answer.answer.clone();
                ctx.execute_activity(
                    FactoryActivities::answer_gate,
                    GateAnswered { key, answer },
                    record_options,
                )
                .await?;
                return Ok(outcome);
            }
            Some(Taken::Reply(reply)) => {
                gate.close(ctx);
                let reading = ReplyReading {
                    key,
                    question: gate.question,
                    reply: reply.reply,
                    user_id: reply.user_id,
                };
                let read = ctx
                    .execute_activity(
                        GateActivities::read_gate_reply,
                        reading,
                        record_options.clone(),
                    )
                    .await?;
                match read {
                    ReplyRead::Answered(outcome) => return Ok(outcome),
                    // The factory asked back on the Gate, which waits
                    // again for an answer or another reply.
                    ReplyRead::AskedBack => gate.open(ctx),
                }
            }
        }
    }
}

/// Tell the Run's owner of the Gate of `key`, which waits for a person.
/// Telling them is tried for at most `NOTIFY_TOTAL`, and a notice that
/// never went out leaves the Gate waiting all the same: it is listed as
/// waiting on them. A Run whose history waited on this Gate before the
/// marker existed replays without telling.
async fn tell_owner(ctx: &WorkflowContext<FlowWorkflow>, key: StepKey) {
    if ctx.patched(GATE_OWNER_PATCH) {
        let options =
            ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::ScheduleAndStartToClose {
                schedule_to_close: NOTIFY_TOTAL,
                start_to_close: RECORD_TIMEOUT,
            })
            .build();
        let _ = ctx
            .execute_activity(FactoryActivities::notify_gate, key, options)
            .await;
    }
}

/// The Gate a Run waits on, as the workflow keeps it.
struct Waiting {
    /// The Step that asked.
    key: StepKey,
    /// Whether it is an agent's question.
    question: bool,
    /// The answers it offers.
    answers: Vec<Outcome>,
    /// Whether it takes replies: false for a Gate opened before them.
    replies: bool,
}

/// What a person gave a waiting Gate.
enum Taken {
    /// One of its answers.
    Answer(AnswerGate),
    /// Their words.
    Reply(ReplyGate),
}

impl Waiting {
    /// Take answers and replies to the Gate.
    fn open(&self, ctx: &WorkflowContext<FlowWorkflow>) {
        ctx.state_mut(|workflow| {
            workflow.open = Some(OpenGate {
                seq: self.key.seq,
                answers: self.answers.clone(),
                replies: self.replies,
            });
            workflow.answer = None;
            workflow.reply = None;
        });
    }

    /// Take no more answers or replies.
    fn close(&self, ctx: &WorkflowContext<FlowWorkflow>) {
        ctx.state_mut(|workflow| workflow.open = None);
    }

    /// The answer or the reply the Gate took, if any. The Gate closes in
    /// the same change that takes it, so no second one slips in between.
    fn take(&self, ctx: &WorkflowContext<FlowWorkflow>) -> Option<Taken> {
        ctx.state_mut(|workflow| {
            let taken = match (workflow.answer.take(), workflow.reply.take()) {
                (Some(answer), _) => Some(Taken::Answer(answer)),
                (None, Some(reply)) => Some(Taken::Reply(reply)),
                (None, None) => None,
            };
            if taken.is_some() {
                workflow.open = None;
            }
            taken
        })
    }
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
