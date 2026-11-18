//! Gates: a Run waiting for a person's answer, and which answers it takes.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, StepKey};
use factory_records::domain::standing_acts::{StandingAct, StandingActor};
use factory_records::domain::standing_rules::RuleGateKind;
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The Gate a Run waits on: the Step that asked, and the answers it offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenGate {
    /// The place of the Step that asked, among the Run's Steps.
    pub seq: u16,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
    /// Whether it takes a person's words as a reply: a Gate opened before
    /// the workflow read replies takes only answers.
    #[serde(default)]
    pub replies: bool,
}

/// A person's answer to the Gate of one of a Run's Steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerGate {
    /// The place of the Step that asked, among the Run's Steps.
    pub seq: u16,
    /// The answer, one the Gate offers.
    pub answer: Outcome,
    /// What the person adds, for the Run's later Steps to read.
    pub note: Option<String>,
    /// Who answered.
    pub user_id: MacroUserIdStr<'static>,
    /// The Run's Tampered files the person approved with the answer; none
    /// when it approved none. Answers sent before Runs were checked for
    /// Control Files carry none.
    #[serde(default)]
    pub approved_control_files: Option<Vec<String>>,
    /// The agent session that gave the answer for the person, when the
    /// Orchestrator did. Left out when none, so answers read and recorded
    /// before it are the same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<Uuid>,
    /// The bot of that session, its Agent and the Standing Role that let it
    /// answer, when the Run's Factory Definition has Standing Roles. Left
    /// out when none, so answers read and recorded before it are the same:
    /// the workflow only hands it on to the activity that records it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing: Option<StandingAnswer>,
}

/// The Standing Role an agent answered a Gate under, for the worker to
/// keep with the answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandingAnswer {
    /// The act: answering a plan or a Blockers Gate.
    pub act: StandingAct,
    /// The session, its bot and Agent, and the Standing Role.
    pub by: StandingActor,
}

/// A Gate a Run's workflow opened, for its worker to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateOpening {
    /// The Step that asked.
    pub key: StepKey,
    /// The question.
    pub question: String,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
}

/// A notify-only Gate a Run's workflow passed, for its worker to tell the
/// Run's owner of: the Run does not wait on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateTelling {
    /// The Step whose end opened it.
    pub key: StepKey,
    /// What it tells, from the Flow.
    pub question: String,
}

/// An answer a Run's workflow took, for its worker to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateAnswered {
    /// The Step that asked.
    pub key: StepKey,
    /// The answer.
    pub answer: AnswerGate,
}

/// A Gate a Run's workflow opened, for its worker to ask the Run's
/// Standing Rules about before a person may answer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRuling {
    /// The Step that asked.
    pub key: StepKey,
    /// The Gate's kind, whose rule may answer it.
    pub kind: RuleGateKind,
    /// For a review panel's Gate, the fix rounds in a row its Blockers went
    /// to since the panel's visits last started; nought for others.
    pub fix_rounds: u16,
    /// For a review panel's Gate, the fix rounds in a row its rule may
    /// send; nought for others.
    pub fix_limit: u16,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
}

/// A person's words on the Gate a Run waits on, in place of one of its
/// answers: the factory reads them as one, or asks back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyGate {
    /// The place of the Step that asked, among the Run's Steps.
    pub seq: u16,
    /// The person's words.
    pub reply: String,
    /// Who wrote them.
    pub user_id: MacroUserIdStr<'static>,
}

/// A reply a Run's workflow took, for its worker to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyReading {
    /// The Step that asked.
    pub key: StepKey,
    /// Whether the Gate is an agent's question, whose answer is the words
    /// themselves.
    pub question: bool,
    /// The person's words.
    pub reply: String,
    /// Who wrote them.
    pub user_id: MacroUserIdStr<'static>,
}

/// How the worker read a reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplyRead {
    /// The Gate is answered: with this answer, which the Run goes on with.
    Answered(Outcome),
    /// The factory asked back on the Gate, which still waits.
    AskedBack,
}

/// A Gate no Standing Rule answered, for its worker to ask the stand-in
/// about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandInAsking {
    /// The Step that asked.
    pub key: StepKey,
    /// The Gate's kind, from its Flow.
    pub kind: RuleGateKind,
    /// Whether its Flow keeps it for a person only.
    pub person_only: bool,
    /// For a review panel's Gate, the fix rounds in a row its Blockers
    /// went to; nought for other Gates.
    #[serde(default)]
    pub fix_rounds: u16,
    /// The fix rounds in a row the `fix_blockers` rule may send; nought,
    /// which leaves a Blockers Gate to the owner, when not given.
    #[serde(default)]
    pub fix_limit: u16,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
}

/// A Gate whose stand-in answer's window ended, for its worker to keep
/// the answer, unless the Run now forbids the stand-in to answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandInKeeping {
    /// The Step that asked.
    pub key: StepKey,
    /// The Gate's kind, from its Flow.
    pub kind: RuleGateKind,
    /// Whether its Flow keeps it for a person only.
    pub person_only: bool,
    /// As for [`StandInAsking::fix_rounds`].
    #[serde(default)]
    pub fix_rounds: u16,
    /// As for [`StandInAsking::fix_limit`].
    #[serde(default)]
    pub fix_limit: u16,
}

/// The stand-in's answer to a Gate, as the workflow goes on with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandingIn {
    /// Its answer, one the Gate offers.
    pub answer: Outcome,
    /// How long, in seconds, the owner may override it before the Run goes
    /// on with it.
    pub window_secs: u32,
}

/// Why a Run does not take an answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateRefusal {
    /// The Run is not waiting on this Gate: it waits on none, or on another.
    #[error("the run is not waiting on a gate of step {seq}")]
    NotWaiting {
        /// The Step the answer names.
        seq: u16,
    },
    /// The Gate already has its answer.
    #[error("the gate already has an answer")]
    AlreadyAnswered,
    /// The Gate does not offer this answer.
    #[error("the gate does not offer the answer {0}")]
    UnknownAnswer(Outcome),
    /// The Gate takes only one of its answers: it opened before the Run
    /// read replies.
    #[error("the gate takes only one of its answers")]
    NoReplies,
}

/// Whether a Run waiting on `open`, already `answered` or not, takes
/// `answer`.
pub fn check_answer(
    open: Option<&OpenGate>,
    answered: bool,
    answer: &AnswerGate,
) -> Result<(), GateRefusal> {
    let Some(gate) = open.filter(|gate| gate.seq == answer.seq) else {
        return Err(GateRefusal::NotWaiting { seq: answer.seq });
    };

    if answered {
        return Err(GateRefusal::AlreadyAnswered);
    }
    if !gate.answers.contains(&answer.answer) {
        return Err(GateRefusal::UnknownAnswer(answer.answer.clone()));
    }
    Ok(())
}

/// Whether a Run waiting on `open`, already given an answer or a reply
/// (`taken`) or not, takes `reply`.
pub fn check_reply(
    open: Option<&OpenGate>,
    taken: bool,
    reply: &ReplyGate,
) -> Result<(), GateRefusal> {
    let Some(gate) = open.filter(|gate| gate.seq == reply.seq) else {
        return Err(GateRefusal::NotWaiting { seq: reply.seq });
    };

    if taken {
        return Err(GateRefusal::AlreadyAnswered);
    }
    if !gate.replies {
        return Err(GateRefusal::NoReplies);
    }
    Ok(())
}
