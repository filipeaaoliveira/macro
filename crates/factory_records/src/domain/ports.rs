//! The port the factory's records are kept behind.

use crate::domain::models::{EntryId, Outcome, Run, RunId, RunOutcome, Step};

/// Where Runs and their Steps are kept.
///
/// A Run's workflow repeats its writes when Temporal retries them, so every
/// write that a Step or a Run's end makes is safe to repeat.
pub trait RunRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Keep a new Run that has not started yet.
    fn insert_run(
        &self,
        id: RunId,
        request: &str,
    ) -> impl Future<Output = Result<Run, Self::Err>> + Send;

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

    /// Record how the Run ended. A Run that already ended keeps its end.
    fn end_run(
        &self,
        id: RunId,
        outcome: RunOutcome,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
