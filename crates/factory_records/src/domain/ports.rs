//! The port the factory's records are kept behind.

use std::collections::BTreeMap;

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{
    ClaimedSession, EntryId, HandBack, Outcome, Run, RunId, RunInsert, RunOutcome, SessionStep,
    Step, StepKey, StepSession, StoppingSession, TaskLink,
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

    /// The newest `limit` Runs of `user_id` that work on the task document
    /// `task_id`, newest first.
    fn list_task_runs(
        &self,
        task_id: Uuid,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> impl Future<Output = Result<Vec<Run>, Self::Err>> + Send;

    /// The Steps of the Runs `ids`, each Run's in order. A Run with no Steps
    /// has no entry.
    fn list_runs_steps(
        &self,
        ids: &[RunId],
    ) -> impl Future<Output = Result<BTreeMap<RunId, Vec<Step>>, Self::Err>> + Send;

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

    /// Record that an attempt of the Step `key` is about to ask for its
    /// session to be opened, at the database's time. False once the Step
    /// let go of the session, or it or its Run ended: no prepare may be sent
    /// then. The time recorded, for [`Self::forget_prepare`]; `None` when no
    /// prepare may be sent.
    fn record_prepare(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Option<chrono::DateTime<chrono::Utc>>, Self::Err>> + Send;

    /// Record that the prepare of the Step `key`'s session recorded at
    /// `prepared_at` is not in flight: it was refused, or never showed the
    /// session open. A later attempt's prepare, and a session an attempt saw
    /// open, keep their record.
    fn forget_prepare(
        &self,
        key: StepKey,
        prepared_at: chrono::DateTime<chrono::Utc>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that an attempt of the Step `key` saw its session open.
    fn record_open(&self, key: StepKey) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The session the Step `key` claimed, as a stop sees it: whether it
    /// opened, and whether a prepare sent less than `lands_within` ago may
    /// still open it.
    fn stopping_session(
        &self,
        key: StepKey,
        lands_within: std::time::Duration,
    ) -> impl Future<Output = Result<Option<StoppingSession>, Self::Err>> + Send;

    /// Record that the Step `key` let go of its session: stopped it for
    /// good, or is done with it. Recording it again changes nothing.
    fn release_session(&self, key: StepKey) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Whether the Step `key` still holds the session it claimed: neither the
    /// Step nor its Run ended, and the session was not let go. An attempt of
    /// the Step opens or prompts its session only while it does.
    fn holds_session(&self, key: StepKey) -> impl Future<Output = Result<bool, Self::Err>> + Send;

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
