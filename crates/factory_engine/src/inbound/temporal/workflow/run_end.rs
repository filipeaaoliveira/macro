//! Whether a Run's walk ends it `infrastructure` rather than `failed`,
//! decided apart from Temporal so it can be tested: only when the Flow
//! routed `failed` of work that failed for infrastructure, that `failed`
//! itself ended the Run, and the Flow did not stop on an error of its own.
//! A `failed` that took the Run back to an entry out of visits, which then
//! ended it `failed` (`on_exhausted`), is the work's: the loop did not
//! converge.

#[cfg(test)]
mod test;

use factory_records::domain::models::RunOutcome;

use super::review_round::Round;
use crate::domain::interpreter::RunState;

/// What the walk's last routed round says of the Run's end.
#[derive(Debug, Default)]
pub(super) struct RunEnding {
    /// Whether the outcome the Flow routed last came of work that failed
    /// for infrastructure.
    on_infrastructure: bool,
}

impl RunEnding {
    /// Note the round whose outcome the Flow routed, and `state`, where it
    /// took the Run: only a `failed` of infrastructure's that ended the Run
    /// itself counts.
    pub(super) fn routed(&mut self, round: &Round, state: &RunState) {
        self.on_infrastructure =
            *round == Round::FailedForInfrastructure && state.ended() == Some(RunOutcome::Failed);
    }

    /// Whether a Run that ends `outcome` ends `infrastructure` instead:
    /// it ended `failed` on the last routed round, which failed for
    /// infrastructure, and not because the Flow could not go on
    /// (`stopped`).
    pub(super) fn infrastructure(&self, outcome: RunOutcome, stopped: bool) -> bool {
        outcome == RunOutcome::Failed && self.on_infrastructure && !stopped
    }
}
