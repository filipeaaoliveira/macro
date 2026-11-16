//! The port the factory's records are kept behind.

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{
    ClaimedSession, EntryId, HandBack, Outcome, Run, RunId, RunInsert, RunOutcome, SessionStep,
    Step, StepKey, StepSession, TaskLink,
};

/// Where Runs and their Steps are kept.
///
/// A Run's workflow repeats its writes when Temporal retries them, so every
/// write that a Step or a Run's end makes is safe to repeat. Steps are found
/// by their place in the Run ([`StepKey`]), which a reset never reuses.
pub trait RunRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Keep a new Run of `user_id` that has not started yet, working on the
    /// task document `task_id` if one is given, in one write.
    fn insert_run(
        &self,
        id: RunId,
        request: &str,
        user_id: &MacroUserIdStr<'static>,
        task_id: Option<Uuid>,
    ) -> impl Future<Output = Result<RunInsert, Self::Err>> + Send;

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

    /// The Run's Steps, in order.
    fn list_steps(&self, id: RunId) -> impl Future<Output = Result<Vec<Step>, Self::Err>> + Send;

    /// Record that the Step `key`, the `visit` of `entry`, started.
    /// Recording it again changes nothing.
    fn start_step(
        &self,
        key: StepKey,
        entry: &EntryId,
        visit: u16,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record how the Step `key` ended and what it produced. A Step that
    /// already ended keeps its first end.
    fn end_step(
        &self,
        key: StepKey,
        outcome: &Outcome,
        output: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the Step `key` waits for a person to answer `question`
    /// with one of `answers`. Recording it again changes nothing.
    fn open_gate(
        &self,
        key: StepKey,
        question: &str,
        answers: &[Outcome],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record `user_id`'s answer to the Gate of the Step `key`. A Gate
    /// already answered keeps its first answer.
    fn answer_gate(
        &self,
        key: StepKey,
        answer: &Outcome,
        note: Option<&str>,
        user_id: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the Step `key` works in `session`, offering its agent
    /// `offered`, unless an earlier attempt of the Step already recorded a
    /// session: then that one is returned and kept.
    fn claim_session(
        &self,
        key: StepKey,
        session: StepSession,
        offered: &[Outcome],
    ) -> impl Future<Output = Result<ClaimedSession, Self::Err>> + Send;

    /// The session the Step `key` works in, if it claimed one.
    fn claimed_session(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Option<StepSession>, Self::Err>> + Send;

    /// The Step that works in the agent session `session_id`, if any.
    fn session_step(
        &self,
        session_id: Uuid,
    ) -> impl Future<Output = Result<Option<SessionStep>, Self::Err>> + Send;

    /// Record what the agent in `session_id` handed back, replacing an
    /// earlier hand back, while its Step has not ended. Whether it was kept:
    /// false once the Step ended.
    fn hand_back(
        &self,
        session_id: Uuid,
        hand_back: &HandBack,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// What the agent handed back for the Step `key`, if it did.
    fn handed_back(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Option<HandBack>, Self::Err>> + Send;

    /// Record how the Run ended. A Run that already ended keeps its end. A
    /// cancelled Run's Steps that were still open end `cancelled`.
    fn end_run(
        &self,
        id: RunId,
        outcome: RunOutcome,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
