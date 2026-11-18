//! Ports of Gate replies and the stand-in: the small model that reads a
//! person's words or answers for them, the comments on a Run's task, and
//! the activities a Run's workflow asks of its worker.

use chrono::{DateTime, Utc};
use factory_records::domain::models::{Outcome, StepKey};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::gate::{
    CommentsRead, CommentsReading, ReplyRead, ReplyReading, StandInAsking, StandingIn,
};
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

    /// Ask the model `question`, billed to `owner`, the Run's owner.
    fn pick(
        &self,
        owner: &MacroUserIdStr<'static>,
        question: &ModelQuestion,
    ) -> impl Future<Output = Result<ModelPick, Self::Err>> + Send;
}

/// A comment on a task, as a reply to a Gate reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskComment {
    /// What it says.
    pub text: String,
    /// When it was written.
    pub created_at: DateTime<Utc>,
}

/// The comments on the task a Run works on.
pub trait TaskComments: Send + Sync + 'static {
    /// Why a read failed.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The comments `owner` wrote themselves on the task `task_id` after
    /// `after`, oldest first: none an agent or a program wrote for them.
    fn owner_comments_after(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        after: DateTime<Utc>,
    ) -> impl Future<Output = Result<Vec<TaskComment>, Self::Err>> + Send;
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

    /// Answer the Gate of `key` with the stand-in's answer, once the owner
    /// let it stand, and return the answer the Gate keeps.
    fn keep_stand_in(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Outcome, StepError>> + Send;

    /// Look at the Run's task for its owner's comments since the last look.
    fn read_comments(
        &self,
        reading: CommentsReading,
    ) -> impl Future<Output = Result<CommentsRead, StepError>> + Send;
}
