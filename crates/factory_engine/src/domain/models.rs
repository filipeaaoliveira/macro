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
