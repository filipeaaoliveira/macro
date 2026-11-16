//! The port the factory's records are kept behind.

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{EntryId, Outcome, Run, RunId, RunOutcome, Step, TaskLink};

/// Where Runs and their Steps are kept.
///
/// A Run's workflow repeats its writes when Temporal retries them, so every
/// write that a Step or a Run's end makes is safe to repeat.
pub trait RunRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Keep a new Run of `user_id` that has not started yet.
    fn insert_run(
        &self,
        id: RunId,
        request: &str,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<Run, Self::Err>> + Send;

    /// Link the Run to the task document `task_id`, replacing any task it
    /// had. Linking it again changes nothing.
    fn link_task(
        &self,
        id: RunId,
        task_id: Uuid,
    ) -> impl Future<Output = Result<TaskLink, Self::Err>> + Send;

    /// Remove a Run whose workflow never started, with its Steps.
    fn delete_run(&self, id: RunId) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The Run, if it exists.
    fn get_run(&self, id: RunId) -> impl Future<Output = Result<Option<Run>, Self::Err>> + Send;

    /// The Run's Steps, in the order they started.
    fn list_steps(&self, id: RunId) -> impl Future<Output = Result<Vec<Step>, Self::Err>> + Send;

    /// Record that the Run's `visit` of `entry` started. Recording it again
    /// changes nothing.
    fn start_step(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record how the Run's `visit` of `entry` ended and what it produced.
    fn end_step(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
        outcome: &Outcome,
        output: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the Run's `visit` of `entry` waits for a person to answer
    /// `question` with one of `answers`. Recording it again changes nothing.
    fn open_gate(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
        question: &str,
        answers: &[Outcome],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record `user_id`'s answer to the Gate of the Run's `visit` of
    /// `entry`. A Gate already answered keeps its first answer.
    fn answer_gate(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
        answer: &Outcome,
        note: Option<&str>,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record how the Run ended. A Run that already ended keeps its end.
    fn end_run(
        &self,
        id: RunId,
        outcome: RunOutcome,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}

/// The people Macro knows.
pub trait UserDirectory: Send + Sync + 'static {
    /// The error type returned when the directory cannot be read.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Whether `user_id` names an account.
    fn user_exists(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;
}
