//! In-memory stand-ins for Gate replies: the records and a scripted Gate
//! model.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use factory_records::domain::gate_replies::{AnsweredGate, GateReplyRepository, PastAnswer};
use factory_records::domain::models::{
    AskedBack, EntryId, GateAnswer, GateModelReading, Outcome, RunId, StandInAnswer, StepKey,
};
use macro_user_id::user_id::MacroUserIdStr;

use super::{FakeError, FakeRepo};
use crate::domain::ports::{GateModel, ModelPick, ModelQuestion};

impl GateReplyRepository for FakeRepo {
    type Err = FakeError;

    async fn ask_back(
        &self,
        key: StepKey,
        reply: &str,
        reading: &GateModelReading,
    ) -> Result<bool, FakeError> {
        let asked = self.with_step(key, |step| {
            let gate = step.gate.as_mut().filter(|gate| gate.answer.is_none())?;
            gate.asked_back = Some(AskedBack {
                reply: reply.to_string(),
                at: Utc::now(),
                reading: Some(reading.clone()),
            });
            Some(())
        });
        Ok(asked.flatten().is_some())
    }

    async fn decline_stand_in(
        &self,
        key: StepKey,
        reading: &GateModelReading,
    ) -> Result<(), FakeError> {
        self.with_step(key, |step| {
            if let Some(gate) = step.gate.as_mut()
                && gate.answer.is_none()
                && gate.stand_in.is_none()
            {
                gate.stand_in_declined = Some(reading.clone());
            }
        });
        Ok(())
    }

    async fn answer_gate_from_reply(
        &self,
        key: StepKey,
        answer: &Outcome,
        reply: &str,
        user_id: &MacroUserIdStr<'static>,
        confidence: Option<u8>,
    ) -> Result<Option<GateAnswer>, FakeError> {
        let kept = self.with_step(key, |step| {
            let gate = step.gate.as_mut()?;
            if gate.answer.is_none() {
                gate.answer = Some(GateAnswer {
                    answer: answer.clone(),
                    note: Some(reply.to_string()),
                    user_id: Some(user_id.clone()),
                    rule: None,
                    approved_control_files: None,
                    reply_confidence: confidence,
                    by_stand_in: false,
                    agent_session_id: None,
                    answered_at: Utc::now(),
                });
            }
            gate.answer.clone()
        });
        Ok(kept.flatten())
    }

    async fn stand_in(
        &self,
        key: StepKey,
        answer: &StandInAnswer,
    ) -> Result<Option<StandInAnswer>, FakeError> {
        let kept = self.with_step(key, |step| {
            let gate = step.gate.as_mut().filter(|gate| gate.answer.is_none())?;
            if gate.stand_in.is_none() {
                gate.stand_in_declined = None;
            }
            Some(gate.stand_in.get_or_insert_with(|| answer.clone()).clone())
        });
        Ok(kept.flatten())
    }

    async fn answer_gate_by_stand_in(&self, key: StepKey) -> Result<Option<GateAnswer>, FakeError> {
        let kept = self.with_step(key, |step| {
            let gate = step.gate.as_mut()?;
            if gate.answer.is_none()
                && let Some(stand_in) = &gate.stand_in
            {
                gate.answer = Some(GateAnswer {
                    answer: stand_in.answer.clone(),
                    note: None,
                    user_id: None,
                    rule: None,
                    approved_control_files: None,
                    reply_confidence: None,
                    by_stand_in: true,
                    agent_session_id: None,
                    answered_at: Utc::now(),
                });
            }
            gate.answer.clone()
        });
        Ok(kept.flatten())
    }

    async fn past_answers(
        &self,
        owner: &MacroUserIdStr<'static>,
        entry: &EntryId,
        of: RunId,
        limit: u16,
    ) -> Result<Vec<PastAnswer>, FakeError> {
        let runs = self.runs.lock().expect("runs");
        let flows = self.flows.lock().expect("flows");
        let flow = flows.get(&of);
        let steps = self.steps.lock().expect("steps");
        let mut past: Vec<(DateTime<Utc>, PastAnswer)> = steps
            .iter()
            .filter(|(run, step)| {
                let question = step.end.as_ref().is_some_and(|end| {
                    end.outcome == Outcome::question() || end.outcome == Outcome::no_result()
                });
                *run != of
                    && flow.is_some()
                    && flows.get(run) == flow
                    && step.entry == *entry
                    && !question
                    && runs.get(run).is_some_and(|run| run.user_id == *owner)
            })
            .filter_map(|(_, step)| {
                let gate = step.gate.as_ref()?;
                let answer = gate.answer.as_ref().filter(|answer| {
                    answer.user_id.as_ref() == Some(owner) && answer.agent_session_id.is_none()
                })?;
                Some((
                    answer.answered_at,
                    PastAnswer {
                        question: gate.question.clone(),
                        answer: answer.answer.clone(),
                        note: answer.note.clone(),
                    },
                ))
            })
            .collect();
        past.sort_by(|left, right| right.0.cmp(&left.0));
        Ok(past
            .into_iter()
            .take(usize::from(limit))
            .map(|(_, past)| past)
            .collect())
    }

    async fn withdraw_stand_in(&self, key: StepKey) -> Result<(), FakeError> {
        self.with_step(key, |step| {
            if let Some(gate) = step.gate.as_mut()
                && gate.answer.is_none()
            {
                gate.stand_in = None;
            }
        });
        Ok(())
    }

    async fn answered_gates(&self, _limit: u32) -> Result<Vec<AnsweredGate>, FakeError> {
        Ok(Vec::new())
    }
}

/// A Gate model that gives the picks a test scripts, in order, and keeps
/// what it was asked; with none left, or told to, it fails.
#[derive(Clone, Default)]
pub(crate) struct FakeGateModel {
    pub(crate) picks: Arc<Mutex<VecDeque<Result<ModelPick, FakeError>>>>,
    pub(crate) asked: Arc<Mutex<Vec<ModelQuestion>>>,
    /// The Runs each call was about, in order; none for a replay.
    pub(crate) runs: Arc<Mutex<Vec<Option<RunId>>>>,
    /// Never answers, as a model that hangs.
    pub(crate) hangs: bool,
}

impl FakeGateModel {
    /// A model that picks `choice` (none for `unclear`) as sure as
    /// `confidence`, once.
    pub(crate) fn picking(choice: Option<&str>, confidence: f64) -> Self {
        let model = Self::default();
        model.then(choice, confidence);
        model
    }

    /// Pick `choice` as sure as `confidence` next.
    pub(crate) fn then(&self, choice: Option<&str>, confidence: f64) {
        self.picks.lock().expect("picks").push_back(Ok(ModelPick {
            choice: choice.map(|choice| Outcome::try_from(choice.to_string()).expect("valid")),
            confidence,
            reason: "The owner approved every plan like this one.".to_string(),
        }));
    }

    /// A model whose next call fails.
    pub(crate) fn failing() -> Self {
        let model = Self::default();
        model.picks.lock().expect("picks").push_back(Err(FakeError));
        model
    }

    pub(crate) fn asked(&self) -> Vec<ModelQuestion> {
        self.asked.lock().expect("asked").clone()
    }
}

impl GateModel for FakeGateModel {
    type Err = FakeError;

    async fn pick(
        &self,
        _owner: &MacroUserIdStr<'static>,
        run_id: Option<RunId>,
        question: &ModelQuestion,
    ) -> Result<ModelPick, FakeError> {
        self.asked.lock().expect("asked").push(question.clone());
        self.runs.lock().expect("runs").push(run_id);
        if self.hangs {
            std::future::pending::<()>().await;
        }
        let next = self.picks.lock().expect("picks").pop_front();
        next.unwrap_or(Err(FakeError))
    }
}
