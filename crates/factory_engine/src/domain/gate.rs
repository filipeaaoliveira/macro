//! Gates: a Run waiting for a person's answer, and which answers it takes.

#[cfg(test)]
mod test;

use factory_records::domain::models::{EntryId, Outcome, RunId};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};

/// The Gate a Run waits on: the Step that asked, and the answers it offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenGate {
    /// The entry whose Step asked.
    pub entry: EntryId,
    /// Which visit of the entry asked.
    pub visit: u16,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
}

/// A person's answer to the Gate of one visit of an entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerGate {
    /// The entry whose Step asked.
    pub entry: EntryId,
    /// Which visit of the entry asked.
    pub visit: u16,
    /// The answer, one the Gate offers.
    pub answer: Outcome,
    /// What the person adds, for the Run's later Steps to read.
    pub note: Option<String>,
    /// Who answered.
    pub user_id: MacroUserIdStr<'static>,
}

/// A Gate a Run's workflow opened, for its worker to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateOpening {
    /// The Run.
    pub run_id: RunId,
    /// The entry whose Step asked.
    pub entry: EntryId,
    /// Which visit of the entry asked.
    pub visit: u16,
    /// The question.
    pub question: String,
    /// The answers it offers.
    pub answers: Vec<Outcome>,
}

/// An answer a Run's workflow took, for its worker to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateAnswered {
    /// The Run.
    pub run_id: RunId,
    /// The answer.
    pub answer: AnswerGate,
}

/// Why a Run does not take an answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateRefusal {
    /// The Run is not waiting on this Gate: it waits on none, or on another.
    #[error("the run is not waiting on a gate of entry {entry}, visit {visit}")]
    NotWaiting {
        /// The entry the answer names.
        entry: EntryId,
        /// The visit the answer names.
        visit: u16,
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
    let waiting = open.filter(|gate| gate.entry == answer.entry && gate.visit == answer.visit);
    let Some(gate) = waiting else {
        return Err(GateRefusal::NotWaiting {
            entry: answer.entry.clone(),
            visit: answer.visit,
        });
    };

    if answered {
        return Err(GateRefusal::AlreadyAnswered);
    }
    if !gate.answers.contains(&answer.answer) {
        return Err(GateRefusal::UnknownAnswer(answer.answer.clone()));
    }
    Ok(())
}
