//! Gates: a Run waiting for a person's answer, and which answers it takes.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, StepKey};
use factory_records::domain::standing_rules::GateKind;
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};

/// The Gate a Run waits on: the Step that asked, and the answers it offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenGate {
    /// The place of the Step that asked, among the Run's Steps.
    pub seq: u16,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
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
    pub kind: GateKind,
    /// For a review panel's Gate, the fix rounds in a row its Blockers went
    /// to since the panel's visits last started; nought for others.
    pub fix_rounds: u16,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
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
