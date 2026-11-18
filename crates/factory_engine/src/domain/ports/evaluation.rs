//! Ports of Scorers and Benchmarks: the model a Scorer judges with, and
//! the activity a Run's workflow asks of its worker once the Run ended.

use factory_records::domain::models::RunId;
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::models::StepError;

/// What a Scorer's model is asked: to score what `prompt` gives it on the
/// one dimension `instructions` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JudgeQuestion {
    /// What the model is told: the Scorer's instructions, and that what it
    /// reads is data.
    pub instructions: String,
    /// What it reads: the Run, as one JSON object.
    pub prompt: String,
}

/// What a Scorer's model answered.
#[derive(Debug, Clone, PartialEq)]
pub struct JudgeVerdict {
    /// From 0 to 1.
    pub score: f64,
    /// Why, in a sentence or two.
    pub reason: String,
}

/// A small, cheap model that scores a finished Run for a Scorer.
pub trait RunJudge: Send + Sync + 'static {
    /// Why a call failed.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The model's id, as its judgments record it.
    fn model(&self) -> &str;

    /// Ask the model `question`, its usage recorded against `owner`, the
    /// Run's owner, and billed to no one.
    fn judge(
        &self,
        owner: &MacroUserIdStr<'static>,
        question: &JudgeQuestion,
    ) -> impl Future<Output = Result<JudgeVerdict, Self::Err>> + Send;
}

/// What a Run's worker does once the Run ended, safe to repeat: a retry
/// finds what an earlier attempt recorded and judges nothing twice.
pub trait ScoringService: Send + Sync + 'static {
    /// Score the Run that ended: a trial gets its result and its
    /// Benchmark's Scorers' judgments; any other Run, the judgments of the
    /// Scorers whose sample takes it.
    fn score_run(&self, run_id: RunId) -> impl Future<Output = Result<(), StepError>> + Send;
}
