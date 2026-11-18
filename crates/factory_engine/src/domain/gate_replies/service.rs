//! What a Run's worker does for Gate replies and the stand-in, over the
//! Run records, the Gate model and the comments on the Run's task.

use chrono::Utc;
use factory_records::domain::gate_replies::GateReplyRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{Gate, MAX_NOTE_BYTES, Outcome, Run, Step, StepKey};
use factory_records::domain::ports::RunRepository;

use super::{Decision, GateReplySettings, MODEL_TIMEOUT, decide, reply_question};
use crate::domain::built_in_steps::cut;
use crate::domain::gate::{
    CommentsRead, CommentsReading, ReplyGate, ReplyRead, ReplyReading, StandInAsking, StandingIn,
};
use crate::domain::models::StepError;
use crate::domain::ports::{GateModel, GateReplyService, ModelPick, ModelQuestion, TaskComments};
use crate::domain::stand_in::{
    PAST_ANSWERS_READ, StandInContext, StandInFacts, may_stand_in, stand_in_question,
};

/// The answer that agrees to what a Gate asks, which on a Tampered Run
/// also approves its Control Files: a person approves those with the
/// button, which lists them, never in words.
const APPROVE: &str = "approve";

/// The Role whose Step makes a Run's plan.
const PLANNER: &str = "planner";

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> StepError {
    StepError::Internal(rootcause::Report::new(error).into_dynamic())
}

/// Gate replies and the stand-in, over the Run records `repo`, the small
/// model `model` and the Run tasks' `comments`.
#[derive(Debug, Clone)]
pub struct GateRepliesImpl<R, M, C> {
    repo: R,
    model: M,
    comments: C,
    settings: GateReplySettings,
}

impl<R, M, C> GateRepliesImpl<R, M, C>
where
    R: RunRepository + GateReplyRepository + MainHoldRepository,
    M: GateModel,
    C: TaskComments,
{
    /// Create the service, set as `settings` say.
    pub fn new(repo: R, model: M, comments: C, settings: GateReplySettings) -> Self {
        Self {
            repo,
            model,
            comments,
            settings,
        }
    }

    async fn run(&self, key: StepKey) -> Result<Run, StepError> {
        self.repo
            .get_run(key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)
    }

    async fn steps(&self, key: StepKey) -> Result<Vec<Step>, StepError> {
        self.repo.list_steps(key.run_id).await.map_err(internal)
    }

    /// The model's pick for `question`, billed to `run`'s owner; none when
    /// the call failed or took longer than [`MODEL_TIMEOUT`], which decides
    /// nothing.
    async fn pick(&self, run: &Run, question: &ModelQuestion) -> Option<ModelPick> {
        let called = tokio::time::timeout(MODEL_TIMEOUT, self.model.pick(&run.user_id, question));
        match called.await {
            Ok(Ok(pick)) => Some(pick),
            Ok(Err(error)) => {
                tracing::warn!(?error, "the gate model failed: it decides nothing");
                None
            }
            Err(_) => {
                tracing::warn!("the gate model took too long: it decides nothing");
                None
            }
        }
    }

    /// Answer the Gate of `key` with `answer`, read from `reading`'s words,
    /// and return the answer it keeps.
    async fn answer_from_reply(
        &self,
        key: StepKey,
        answer: &Outcome,
        reading: &ReplyReading,
        confidence: Option<u8>,
    ) -> Result<ReplyRead, StepError> {
        let kept = self
            .repo
            .answer_gate_from_reply(key, answer, &reading.reply, &reading.user_id, confidence)
            .await
            .map_err(internal)?
            .ok_or_else(|| StepError::Internal(rootcause::report!("the gate is not recorded")))?;
        Ok(ReplyRead::Answered(kept.answer))
    }
}

/// The Gate of the Step at `seq` among `steps`, which must be recorded.
fn gate_of(steps: &[Step], seq: u16) -> Result<&Gate, StepError> {
    steps
        .iter()
        .find(|step| step.seq == seq)
        .and_then(|step| step.gate.as_ref())
        .ok_or_else(|| StepError::Internal(rootcause::report!("the gate is not recorded")))
}

impl<R, M, C> GateReplyService for GateRepliesImpl<R, M, C>
where
    R: RunRepository + GateReplyRepository + MainHoldRepository,
    M: GateModel,
    C: TaskComments,
{
    #[tracing::instrument(err, skip_all, fields(run_id = %reading.key.run_id, seq = reading.key.seq))]
    async fn read_reply(&self, reading: ReplyReading) -> Result<ReplyRead, StepError> {
        let key = reading.key;
        let steps = self.steps(key).await?;
        let gate = gate_of(&steps, key.seq)?;
        // An earlier attempt may have answered the Gate before its result
        // was lost: that answer stands.
        if let Some(answer) = &gate.answer {
            return Ok(ReplyRead::Answered(answer.answer.clone()));
        }
        if reading.question {
            return self
                .answer_from_reply(key, &Outcome::answer(), &reading, None)
                .await;
        }

        let run = self.run(key).await?;
        let question = reply_question(&gate.question, &gate.answers, &reading.reply);
        let pick = self.pick(&run, &question).await;
        let decision = decide(pick.as_ref(), &gate.answers, self.settings.reply_threshold);
        // Approving a Tampered Run approves its Control Files, which the
        // person must see listed: words never do it.
        let tampered = run
            .control_files
            .as_ref()
            .is_some_and(|changes| changes.is_tampered());
        match decision {
            Decision::Decided { answer, confidence }
                if !(tampered && answer.as_str() == APPROVE) =>
            {
                tracing::info!(%answer, confidence, "a reply answered a gate");
                self.answer_from_reply(key, &answer, &reading, Some(confidence))
                    .await
            }
            _ => {
                self.repo
                    .ask_back(key, &reading.reply)
                    .await
                    .map_err(internal)?;
                tracing::info!("the factory asked back on a gate");
                Ok(ReplyRead::AskedBack)
            }
        }
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %asking.key.run_id, seq = asking.key.seq, kind = %asking.kind))]
    async fn stand_in(&self, asking: StandInAsking) -> Result<Option<StandingIn>, StepError> {
        let key = asking.key;
        let steps = self.steps(key).await?;
        let gate = gate_of(&steps, key.seq)?;
        // An agent's question is the owner's alone to answer.
        if gate.kind.answered_only_by_a_person() || gate.answer.is_some() {
            return Ok(None);
        }
        let now = Utc::now();
        // An earlier attempt's answer stands, with the window it set.
        if let Some(kept) = &gate.stand_in {
            return Ok(Some(StandingIn {
                answer: kept.answer.clone(),
                window_secs: seconds_until(kept.until, now),
            }));
        }

        let run = self.run(key).await?;
        if run.end.is_some() {
            return Ok(None);
        }
        let held = match &run.repository {
            Some(base) => self.repo.main_hold(base).await.map_err(internal)?.is_some(),
            None => false,
        };
        let facts = StandInFacts {
            kind: asking.kind,
            person_only: asking.person_only,
            tampered: run
                .control_files
                .as_ref()
                .is_some_and(|changes| changes.is_tampered()),
            held,
        };
        let rules = run.standing_rules.map(|rules| rules.definition);
        if !may_stand_in(rules, &facts, &asking.answers) {
            return Ok(None);
        }

        let Some(asked) = steps.iter().find(|step| step.seq == key.seq) else {
            return Ok(None);
        };
        let past = self
            .repo
            .past_answers(&run.user_id, &asked.entry, run.id, PAST_ANSWERS_READ)
            .await
            .map_err(internal)?;
        let plan = steps
            .iter()
            .rev()
            .filter(|step| step.seq < key.seq)
            .find(|step| {
                step.role
                    .as_ref()
                    .is_some_and(|role| role.as_str() == PLANNER)
            })
            .and_then(|step| step.end.as_ref())
            .map(|end| end.output.as_str());
        let context = StandInContext {
            question: &gate.question,
            answers: &gate.answers,
            request: &run.request,
            output: asked.end.as_ref().map_or("", |end| end.output.as_str()),
            plan,
            past: &past,
        };
        let pick = self.pick(&run, &stand_in_question(&context)).await;
        let Decision::Decided { answer, confidence } = decide(
            pick.as_ref(),
            &gate.answers,
            self.settings.stand_in_threshold,
        ) else {
            tracing::info!("the stand-in left a gate to its owner");
            return Ok(None);
        };
        let reason = pick.map(|pick| pick.reason).unwrap_or_default();
        let window = chrono::TimeDelta::from_std(self.settings.stand_in_window)
            .map_err(|error| StepError::Final(rootcause::report!("{error}")))?;
        let proposed = factory_records::domain::models::StandInAnswer {
            answer,
            reason: cut(reason.trim(), MAX_NOTE_BYTES).to_string(),
            confidence,
            until: now + window,
        };
        let kept = self.repo.stand_in(key, &proposed).await.map_err(internal)?;
        Ok(kept.map(|kept| {
            tracing::info!(answer = %kept.answer, confidence = kept.confidence, "the stand-in answered a gate");
            StandingIn {
                answer: kept.answer,
                window_secs: seconds_until(kept.until, now),
            }
        }))
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %key.run_id, seq = key.seq))]
    async fn keep_stand_in(&self, key: StepKey) -> Result<Outcome, StepError> {
        let kept = self
            .repo
            .answer_gate_by_stand_in(key)
            .await
            .map_err(internal)?
            .ok_or_else(|| {
                StepError::Final(rootcause::report!(
                    "the gate has no answer of the stand-in's to keep"
                ))
            })?;
        Ok(kept.answer)
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %reading.key.run_id, seq = reading.key.seq))]
    async fn read_comments(&self, reading: CommentsReading) -> Result<CommentsRead, StepError> {
        let key = reading.key;
        let run = self.run(key).await?;
        let nothing = CommentsRead {
            reply: None,
            read_until: reading.after,
        };
        let Some(task_id) = run.task_id else {
            return Ok(nothing);
        };
        if run.end.is_some() {
            return Ok(nothing);
        }
        let after = match reading.after {
            Some(after) => after,
            None => {
                let steps = self.steps(key).await?;
                gate_of(&steps, key.seq)?.opened_at
            }
        };

        // A task the comments cannot be read from now is looked at again
        // at the next look.
        let comments = match self
            .comments
            .owner_comments_after(&run.user_id, task_id, after)
            .await
        {
            Ok(comments) => comments,
            Err(error) => {
                tracing::warn!(?error, "the run task's comments could not be read");
                return Ok(CommentsRead {
                    reply: None,
                    read_until: Some(after),
                });
            }
        };
        // The newest comment is the owner's latest word; the ones before it
        // are read past.
        let Some(newest) = comments.iter().max_by_key(|comment| comment.created_at) else {
            return Ok(CommentsRead {
                reply: None,
                read_until: Some(after),
            });
        };
        let reply = cut(newest.text.trim(), MAX_NOTE_BYTES).to_string();
        Ok(CommentsRead {
            reply: (!reply.is_empty()).then(|| ReplyGate {
                seq: key.seq,
                reply,
                user_id: run.user_id.clone(),
            }),
            read_until: Some(newest.created_at),
        })
    }
}

/// Whole seconds from `now` until `until`; nought once it passed.
fn seconds_until(until: chrono::DateTime<Utc>, now: chrono::DateTime<Utc>) -> u32 {
    let seconds = (until - now).num_seconds().max(0);
    u32::try_from(seconds).unwrap_or(u32::MAX)
}
