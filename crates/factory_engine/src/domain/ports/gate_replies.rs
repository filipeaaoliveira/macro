//! Ports of Gate replies and the stand-in: the small model that reads a
//! person's words or answers for them, and the activities a Run's
//! workflow asks of its worker.

use factory_records::domain::models::{Outcome, RunId};
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::gate::{ReplyRead, ReplyReading, StandInAsking, StandInKeeping, StandingIn};
use crate::domain::models::StepError;

/// What a Gate's model is asked: to pick one of `choices`, or none, from
/// what `prompt` gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelQuestion {
    /// What the model is told to do, and that what it reads is data.
    pub instructions: &'static str,
    /// What it reads: the Gate, and the words or the Run it answers from.
    pub prompt: String,
    /// The answers it may pick.
    pub choices: Vec<Outcome>,
}

/// What a Gate's model picked.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPick {
    /// The answer it picked; none when it could not tell (`unclear`).
    pub choice: Option<Outcome>,
    /// How sure it was, from 0 to 1.
    pub confidence: f64,
    /// Why, in a sentence or two.
    pub reason: String,
}

/// A small, cheap model that picks one of a Gate's answers. Its pick only
/// counts at or above a threshold the factory sets; a failure decides
/// nothing.
pub trait GateModel: Send + Sync + 'static {
    /// Why a call failed.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Ask the model `question` about a Gate of the Run `run_id`, billed to
    /// `owner`, the Run's owner; its usage names the Run.
    fn pick(
        &self,
        owner: &MacroUserIdStr<'static>,
        run_id: RunId,
        question: &ModelQuestion,
    ) -> impl Future<Output = Result<ModelPick, Self::Err>> + Send;
}

/// What a Run's worker does for Gate replies and the stand-in, each safe
/// to repeat: a retry finds what an earlier attempt recorded.
pub trait GateReplyService: Send + Sync + 'static {
    /// Read a person's reply to a Gate: answer the Gate with what the
    /// words say, or ask back on it.
    fn read_reply(
        &self,
        reading: ReplyReading,
    ) -> impl Future<Output = Result<ReplyRead, StepError>> + Send;

    /// Ask the stand-in about a Gate no Standing Rule answered: keep and
    /// return its answer, or none, for the owner to answer.
    fn stand_in(
        &self,
        asking: StandInAsking,
    ) -> impl Future<Output = Result<Option<StandingIn>, StepError>> + Send;

    /// Answer the Gate with the stand-in's answer once the owner let it
    /// stand, and return the answer the Gate keeps; none when the Run now
    /// forbids the stand-in to answer (Tampered, under a Main Hold, rules
    /// that no longer let it), which takes its answer back, so the Gate
    /// waits for the person.
    fn keep_stand_in(
        &self,
        keeping: StandInKeeping,
    ) -> impl Future<Output = Result<Option<Outcome>, StepError>> + Send;
}
