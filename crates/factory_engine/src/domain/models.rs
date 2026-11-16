//! What the factory's use cases take and return.

use factory_records::domain::models::{EntryId, Outcome, Run, RunId, RunOutcome, Step};
use serde::{Deserialize, Serialize};

use crate::domain::flow::{Flow, FlowError, StepName};

/// What a Run's workflow starts with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStart {
    /// The Run, already recorded.
    pub run_id: RunId,
    /// The Flow it walks.
    pub flow: Flow,
    /// What it was asked to do.
    pub request: String,
}

/// One Step for a worker to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRequest {
    /// The Run the Step belongs to.
    pub run_id: RunId,
    /// The entry it visits.
    pub entry: EntryId,
    /// The work it does.
    pub step: StepName,
    /// Which visit of the entry it is, from 1.
    pub visit: u16,
    /// What the Run was asked to do.
    pub request: String,
}

/// What a Step's work is given: the Run's request, and what the Run's Steps
/// before it produced.
#[derive(Debug, Clone, Copy)]
pub struct StepInput<'a> {
    /// The Run the Step belongs to.
    pub run_id: RunId,
    /// What the Run was asked to do.
    pub request: &'a str,
    /// The Run's Steps that ended before this one, in the order they started.
    pub earlier: &'a [Step],
}

/// What an agent is asked to do for one Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTask {
    /// The Run the Step belongs to.
    pub run_id: RunId,
    /// The Step.
    pub step: StepName,
    /// The Step's instructions, the Run's request and its earlier Steps.
    pub prompt: String,
    /// The outcomes the agent may end the Step with.
    pub outcomes: Vec<Outcome>,
}

/// What an agent handed back for a Step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAnswer {
    /// One of the task's outcomes.
    pub outcome: Outcome,
    /// What it did, for a person and the Run's later Steps to read.
    pub text: String,
}

/// How a Step ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepReport {
    /// Its outcome, which the Flow routes.
    pub outcome: Outcome,
    /// What it produced, for a person to read.
    pub output: String,
}

/// A Run's end, for its workflow to record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFinish {
    /// The Run.
    pub run_id: RunId,
    /// How it ended.
    pub outcome: RunOutcome,
}

/// A Run and the Steps it took.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    /// The Run.
    pub run: Run,
    /// Its Steps, in the order they started.
    pub steps: Vec<Step>,
}

/// Errors of the factory's Run use cases.
#[derive(Debug, thiserror::Error)]
pub enum FactoryError {
    /// The Flow definition does not hold together.
    #[error(transparent)]
    InvalidFlow(#[from] FlowError),
    /// The request is empty.
    #[error("the request must not be empty")]
    EmptyRequest,
    /// The request is larger than a Run keeps.
    #[error("the request must be at most {max} bytes")]
    RequestTooLarge {
        /// The most bytes a request may hold.
        max: usize,
    },
    /// No Run has this id.
    #[error("run not found")]
    NotFound,
    /// The Run has ended, so it takes no answer and cannot be cancelled.
    #[error("the run has ended")]
    RunEnded,
    /// The note on an answer is larger than a Gate keeps.
    #[error("the note must be at most {max} bytes")]
    NoteTooLarge {
        /// The most bytes a note may hold.
        max: usize,
    },
    /// The Run's workflow did not take the answer, and said why.
    #[error("the answer was refused: {0}")]
    AnswerRefused(String),
    /// Any other internal error.
    #[error("internal factory error: {0:?}")]
    Internal(rootcause::Report),
}

impl From<rootcause::Report> for FactoryError {
    fn from(report: rootcause::Report) -> Self {
        FactoryError::Internal(report)
    }
}

/// Errors of running a Step or recording a Run's end.
#[derive(Debug, thiserror::Error)]
pub enum StepError {
    /// The Step produced more output than a Step keeps.
    #[error("the step's output must be at most {max} bytes")]
    OutputTooLarge {
        /// The most bytes an output may hold.
        max: usize,
    },
    /// Any other internal error.
    #[error("internal step error: {0:?}")]
    Internal(rootcause::Report),
}

impl From<rootcause::Report> for StepError {
    fn from(report: rootcause::Report) -> Self {
        StepError::Internal(report)
    }
}
