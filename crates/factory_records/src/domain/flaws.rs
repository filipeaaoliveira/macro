//! Flawed Runs: a Run that ended `success` whose result a Scorer or a
//! person found wrong afterwards ends `flawed` instead, and keeps who found
//! it so, why and when.

use std::future::Future;

use crate::domain::models::{FlawMarker, RunEnd, RunId, RunOutcome};

/// The most bytes a flaw's reason holds.
pub const MAX_FLAW_REASON_BYTES: usize = 4096;

/// What marking a Run flawed did.
#[derive(Debug, Clone, PartialEq)]
pub enum FlawMarking {
    /// The Run reads as flawed: marked now, or by an earlier mark, which
    /// stands. Its end, with the flaw.
    Flawed(RunEnd),
    /// The Run did not end `success`: it still runs (none), or ended
    /// another way.
    NotSucceeded(Option<RunOutcome>),
    /// No such Run.
    NoSuchRun,
}

/// Marks Runs flawed.
pub trait FlawRepository: Send + Sync + 'static {
    /// The error type returned by writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Mark the Run `id` flawed by `by`, for `reason`, when it ended
    /// `success`. A Run already flawed keeps its first mark, so marking it
    /// again changes nothing. A person's mark must name the Run's owner.
    fn mark_flawed(
        &self,
        id: RunId,
        by: &FlawMarker,
        reason: &str,
    ) -> impl Future<Output = Result<FlawMarking, Self::Err>> + Send;
}
