//! What a Run's worker does for Gate replies and the stand-in, over the
//! Run records and the Gate model.

use chrono::Utc;
use factory_records::domain::gate_replies::GateReplyRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    EntryId, Gate, GateModelReading, MAX_NOTE_BYTES, Outcome, Run, StandInAnswer, Step, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;

use super::{Decision, GateReplySettings, MODEL_TIMEOUT, decide, percent, reply_question};
use crate::domain::built_in_steps::cut;
use factory_records::domain::standing_rules::RuleGateKind;

use crate::domain::gate::{ReplyRead, ReplyReading, StandInAsking, StandInKeeping, StandingIn};
use crate::domain::gate_owner::{notice_id, notice_may_be_out, stand_in_id, stand_in_notice};
use crate::domain::models::StepError;
use crate::domain::plan_review::{blockers_left, earlier_gate_entry};
use crate::domain::ports::{
    GateModel, GateNotifier, GateReplyService, ModelPick, ModelQuestion, NoOwnerGates,
    NoTaskThread, TaskThread,
};
use crate::domain::stand_in::{
    PAST_ANSWERS_READ, StandInContext, StandInFacts, may_stand_in, stand_in_question,
};
use crate::domain::standing_rules::repeated_blockers;
use crate::domain::task_thread::{RunUpdate, post_update};

/// The answer that agrees to what a Gate asks, which on a Tampered Run
/// also approves its Control Files: a person approves those with the
/// button, which lists them, never in words.
const APPROVE: &str = "approve";

/// The Role whose Step makes a Run's plan.
const PLANNER: &str = "planner";

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> StepError {
    StepError::Internal(rootcause::Report::new(error).into_dynamic())
}

/// Gate replies and the stand-in, over the Run records `repo` and the
/// small model `model`. A Gate answered from a person's words has its
/// owner's notification marked done through `notifier`, as an answer
/// given with a button does. A Gate answered from words or by the
/// stand-in is posted to the Run's task thread through `thread`, when the
/// Run's task thread section is on.
#[derive(Debug, Clone)]
pub struct GateRepliesImpl<R, M, N = NoOwnerGates, P = NoTaskThread> {
    repo: R,
    model: M,
    settings: GateReplySettings,
    notifier: N,
    thread: P,
}

impl<R, M> GateRepliesImpl<R, M>
where
    R: RunRepository + GateReplyRepository + MainHoldRepository + ReviewRoundRepository,
    M: GateModel,
{
    /// Create the service, set as `settings` say, which marks no
    /// notification done until [`Self::with_notifier`].
    pub fn new(repo: R, model: M, settings: GateReplySettings) -> Self {
        Self {
            repo,
            model,
            settings,
            notifier: NoOwnerGates,
            thread: NoTaskThread,
        }
    }
}

impl<R, M, N, P> GateRepliesImpl<R, M, N, P>
where
    R: RunRepository + GateReplyRepository + MainHoldRepository + ReviewRoundRepository,
    M: GateModel,
    N: GateNotifier,
    P: TaskThread,
{
    /// The service, marking the owner's notification of a Gate done
    /// through `notifier` once their words answer it.
    pub fn with_notifier<N2: GateNotifier>(self, notifier: N2) -> GateRepliesImpl<R, M, N2, P> {
        GateRepliesImpl {
            repo: self.repo,
            model: self.model,
            settings: self.settings,
            notifier,
            thread: self.thread,
        }
    }

    /// The service, posting the Gates it answers to their Runs' task
    /// threads through `thread`.
    pub fn with_task_thread<P2: TaskThread>(self, thread: P2) -> GateRepliesImpl<R, M, N, P2> {
        GateRepliesImpl {
            repo: self.repo,
            model: self.model,
            settings: self.settings,
            notifier: self.notifier,
            thread,
        }
    }

    /// Post that the Gate of `key` was answered to its Run's task thread.
    /// It never fails the activity that posts it.
    async fn post_answered(&self, key: StepKey) {
        let update = RunUpdate::GateAnswered { seq: key.seq };
        post_update(&self.repo, &self.thread, key.run_id, update).await;
    }

    /// Whether the stand-in may answer the Gate `asked` of `run`, whose
    /// Steps are `steps`, offering `answers`, as the Run reads now.
    async fn may_stand_in_now(
        &self,
        run: &Run,
        steps: &[Step],
        asked: Asked,
        answers: &[Outcome],
    ) -> Result<bool, StepError> {
        if run.end.is_some() {
            return Ok(false);
        }
        let held = match &run.repository {
            Some(base) => self.repo.main_hold(base).await.map_err(internal)?.is_some(),
            None => false,
        };
        // Whether a review round found its round before's Blockers again
        // matters only with the owner section on, as for the rule.
        let owner_gates = run
            .standing_rules
            .is_some_and(|rules| rules.definition.owner_gates);
        let repeated = if asked.kind == RuleGateKind::Blockers && owner_gates {
            let rounds = self
                .repo
                .review_rounds(asked.key.run_id)
                .await
                .map_err(internal)?;
            repeated_blockers(&rounds, steps, asked.key.seq)
        } else {
            false
        };
        let plan_blockers_left = asked.kind == RuleGateKind::Plan
            && steps
                .iter()
                .find(|step| step.seq == asked.key.seq)
                .and_then(|step| step.end.as_ref())
                .is_some_and(|end| blockers_left(&end.output));
        let facts = StandInFacts {
            kind: asked.kind,
            person_only: asked.person_only,
            tampered: run
                .control_files
                .as_ref()
                .is_some_and(|changes| changes.is_tampered()),
            held,
            fix_rounds: asked.fix_rounds,
            fix_limit: asked.fix_limit,
            repeated_blockers: repeated,
            plan_blockers_left,
        };
        let rules = run.standing_rules.map(|rules| rules.definition);
        Ok(may_stand_in(rules, &facts, answers))
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
        let called = tokio::time::timeout(
            MODEL_TIMEOUT,
            self.model.pick(&run.user_id, Some(run.id), question),
        );
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
    /// and return the answer it keeps. `gate` is the Gate as it was read
    /// before: whether the stand-in had answered it, whose window the answer
    /// closes, and whether its owner was told of it.
    async fn answer_from_reply(
        &self,
        key: StepKey,
        answer: &Outcome,
        reading: &ReplyReading,
        confidence: Option<u8>,
        gate: &Gate,
    ) -> Result<ReplyRead, StepError> {
        let kept = self
            .repo
            .answer_gate_from_reply(key, answer, &reading.reply, &reading.user_id, confidence)
            .await
            .map_err(internal)?
            .ok_or_else(|| StepError::Internal(rootcause::report!("the gate is not recorded")))?;
        self.answered(key, gate).await?;
        Ok(ReplyRead::Answered(kept.answer))
    }

    /// The Gate of `key`, `gate` as it was read, is answered: the
    /// notification its owner was told of it by is done, so is the notice
    /// that the stand-in answered, when it had, and the answer is posted to
    /// its task thread, when that section is on. A notice that never went
    /// out is left alone, as closing it would leave a withdrawal behind.
    async fn answered(&self, key: StepKey, gate: &Gate) -> Result<(), StepError> {
        let run = self.run(key).await?;
        if notice_may_be_out(&run, gate) {
            let id = notice_id(key.run_id, key.seq);
            self.notifier.answered(&run.user_id, id).await;
        }
        if gate.stand_in.is_some() {
            self.window_closed(&run, key).await;
        }
        self.post_answered(key).await;
        Ok(())
    }

    /// The stand-in's window on the Gate of `key` closed: the notice that
    /// the stand-in answered is done, a Run's with a task or without one.
    async fn window_closed(&self, run: &Run, key: StepKey) {
        let id = stand_in_id(key.run_id, key.seq);
        self.notifier.answered(&run.user_id, id).await;
    }

    /// Tell the owner of `run` that the stand-in answered the Gate of
    /// `key` with `kept`, which they may change until its window closes.
    async fn tell_stand_in(&self, run: &Run, steps: &[Step], key: StepKey, kept: &StandInAnswer) {
        let Some(step) = steps.iter().find(|step| step.seq == key.seq) else {
            return;
        };
        if let Some(notice) = stand_in_notice(run, step, &kept.answer) {
            self.notifier.notify(notice).await;
        }
    }
}

/// What the stand-in reads of the Gate the workflow asks about.
#[derive(Debug, Clone, Copy)]
struct Asked {
    key: StepKey,
    kind: RuleGateKind,
    person_only: bool,
    fix_rounds: u16,
    fix_limit: u16,
}

impl From<&StandInAsking> for Asked {
    fn from(asking: &StandInAsking) -> Self {
        Self {
            key: asking.key,
            kind: asking.kind,
            person_only: asking.person_only,
            fix_rounds: asking.fix_rounds,
            fix_limit: asking.fix_limit,
        }
    }
}

impl From<&StandInKeeping> for Asked {
    fn from(keeping: &StandInKeeping) -> Self {
        Self {
            key: keeping.key,
            kind: keeping.kind,
            person_only: keeping.person_only,
            fix_rounds: keeping.fix_rounds,
            fix_limit: keeping.fix_limit,
        }
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

impl<R, M, N, P> GateReplyService for GateRepliesImpl<R, M, N, P>
where
    R: RunRepository + GateReplyRepository + MainHoldRepository + ReviewRoundRepository,
    M: GateModel,
    N: GateNotifier,
    P: TaskThread,
{
    #[tracing::instrument(err, skip_all, fields(run_id = %reading.key.run_id, seq = reading.key.seq))]
    async fn read_reply(&self, reading: ReplyReading) -> Result<ReplyRead, StepError> {
        let key = reading.key;
        let steps = self.steps(key).await?;
        let gate = gate_of(&steps, key.seq)?;
        // An earlier attempt may have answered the Gate before its result
        // was lost: that answer stands.
        let stood_in = gate.stand_in.is_some();
        if let Some(answer) = &gate.answer {
            self.answered(key, gate).await?;
            return Ok(ReplyRead::Answered(answer.answer.clone()));
        }
        if reading.question {
            return self
                .answer_from_reply(key, &Outcome::answer(), &reading, None, gate)
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
        let refused_approval = match &decision {
            Decision::Decided { answer, confidence } if !tampered || answer.as_str() != APPROVE => {
                tracing::info!(%answer, confidence, "a reply answered a gate");
                return self
                    .answer_from_reply(key, answer, &reading, Some(*confidence), gate)
                    .await;
            }
            Decision::Decided { .. } => true,
            Decision::Undecided => false,
        };

        let asked = model_reading(pick.as_ref(), &gate.answers);
        self.repo
            .ask_back(key, &reading.reply, &asked, refused_approval)
            .await
            .map_err(internal)?;
        // Words in the stand-in's window end it, whatever they say: the
        // Gate is the owner's again.
        if stood_in {
            self.window_closed(&run, key).await;
        }
        tracing::info!("the factory asked back on a gate");
        Ok(ReplyRead::AskedBack)
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
        let run = self.run(key).await?;
        // An earlier attempt's answer stands, with the window it set; its
        // notice is sent again, which makes one notification.
        if let Some(kept) = &gate.stand_in {
            self.tell_stand_in(&run, &steps, key, kept).await;
            return Ok(Some(StandingIn {
                answer: kept.answer.clone(),
                window_secs: seconds_until(kept.until, now),
            }));
        }

        if !self
            .may_stand_in_now(&run, &steps, Asked::from(&asking), &asking.answers)
            .await?
        {
            return Ok(None);
        }

        let Some(asked) = steps.iter().find(|step| step.seq == key.seq) else {
            return Ok(None);
        };
        let mut past = self
            .repo
            .past_answers(&run.user_id, &asked.entry, run.id, PAST_ANSWERS_READ)
            .await
            .map_err(internal)?;
        // Plan Gates were asked at `plan` before plans were reviewed: the
        // owner's answers there count, after the newer ones.
        let room = usize::from(PAST_ANSWERS_READ).saturating_sub(past.len());
        if room > 0
            && let Some(earlier) = earlier_gate_entry(asked.entry.as_str())
            && let Ok(earlier) = EntryId::try_from(earlier.to_string())
        {
            let older = self
                .repo
                .past_answers(&run.user_id, &earlier, run.id, PAST_ANSWERS_READ)
                .await
                .map_err(internal)?;
            past.extend(older.into_iter().take(room));
        }
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
            // The owner reads on the Gate why it is theirs.
            self.repo
                .decline_stand_in(key, &model_reading(pick.as_ref(), &gate.answers))
                .await
                .map_err(internal)?;
            tracing::info!("the stand-in left a gate to its owner");
            return Ok(None);
        };
        let reason = pick.map(|pick| pick.reason).unwrap_or_default();
        let window = chrono::TimeDelta::from_std(self.settings.stand_in_window)
            .map_err(|error| StepError::Final(rootcause::report!("{error}")))?;
        let proposed = StandInAnswer {
            answer,
            reason: cut(reason.trim(), MAX_NOTE_BYTES).to_string(),
            confidence,
            until: now + window,
        };
        let Some(kept) = self.repo.stand_in(key, &proposed).await.map_err(internal)? else {
            return Ok(None);
        };
        tracing::info!(answer = %kept.answer, confidence = kept.confidence, "the stand-in answered a gate");
        self.tell_stand_in(&run, &steps, key, &kept).await;
        Ok(Some(StandingIn {
            answer: kept.answer,
            window_secs: seconds_until(kept.until, now),
        }))
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %keeping.key.run_id, seq = keeping.key.seq))]
    async fn keep_stand_in(&self, keeping: StandInKeeping) -> Result<Option<Outcome>, StepError> {
        let key = keeping.key;
        let steps = self.steps(key).await?;
        let gate = gate_of(&steps, key.seq)?;
        // An earlier attempt kept it, or the owner answered first: that
        // answer stands, and posting it again posts it once.
        let run = self.run(key).await?;
        self.window_closed(&run, key).await;
        if let Some(answer) = &gate.answer {
            self.post_answered(key).await;
            return Ok(Some(answer.answer.clone()));
        }
        // The Run may have changed in the window: a Tampered branch, a Main
        // Hold, or a Run that ended give the Gate back to the person.
        if !self
            .may_stand_in_now(&run, &steps, Asked::from(&keeping), &gate.answers)
            .await?
        {
            self.repo.withdraw_stand_in(key).await.map_err(internal)?;
            tracing::info!("the run no longer lets the stand-in answer: the gate is its owner's");
            return Ok(None);
        }
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
        self.post_answered(key).await;
        Ok(Some(kept.answer))
    }
}

/// How the Gate model read a Gate offering `offered`, from its `pick`:
/// none when its call failed or took too long, which leaves no pick,
/// confidence or reason. A pick the Gate does not offer is kept as none,
/// as [`decide`] reads it, so the record holds only the Gate's answers.
fn model_reading(pick: Option<&ModelPick>, offered: &[Outcome]) -> GateModelReading {
    match pick {
        Some(pick) => GateModelReading {
            pick: pick
                .choice
                .clone()
                .filter(|choice| offered.contains(choice)),
            confidence: Some(percent(pick.confidence)),
            reason: cut(pick.reason.trim(), MAX_NOTE_BYTES).to_string(),
        },
        None => GateModelReading {
            pick: None,
            confidence: None,
            reason: String::new(),
        },
    }
}

/// Whole seconds from `now` until `until`; nought once it passed.
fn seconds_until(until: chrono::DateTime<Utc>, now: chrono::DateTime<Utc>) -> u32 {
    let seconds = (until - now).num_seconds().max(0);
    u32::try_from(seconds).unwrap_or(u32::MAX)
}
