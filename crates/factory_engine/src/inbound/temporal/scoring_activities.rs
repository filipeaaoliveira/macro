//! The activity a Run's workflow calls once the Run ended, over the scoring
//! service. The judges' model calls run here, and what they decide is only
//! recorded, never read back by the workflow.

use std::sync::Arc;

use factory_records::domain::models::RunId;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

use super::activities::failure;
use super::workflow::SCORE_ATTEMPTS;
use crate::domain::models::StepError;
use crate::domain::ports::ScoringService;

/// The scoring service as a trait object: `#[activities]` takes no generic
/// type, as for the Step service.
#[async_trait::async_trait]
trait DynScoringService: Send + Sync {
    async fn score_run(&self, run_id: RunId, last_attempt: bool) -> Result<(), StepError>;
}

#[async_trait::async_trait]
impl<T: ScoringService> DynScoringService for T {
    async fn score_run(&self, run_id: RunId, last_attempt: bool) -> Result<(), StepError> {
        ScoringService::score_run(self, run_id, last_attempt).await
    }
}

/// The activity of Scorers and Benchmark trials.
pub struct ScoringActivities {
    scoring: Arc<dyn DynScoringService>,
}

impl ScoringActivities {
    /// The activity, over `scoring`.
    pub fn new(scoring: impl ScoringService) -> Self {
        Self {
            scoring: Arc::new(scoring),
        }
    }
}

#[activities]
impl ScoringActivities {
    /// Score a Run that ended: a trial's result and judgments, or the live
    /// Scorers' sampled judgments.
    #[activity(name = "factory_score_run")]
    pub async fn score_run(
        self: Arc<Self>,
        ctx: ActivityContext,
        run_id: RunId,
    ) -> Result<(), ActivityError> {
        let last_attempt = is_last_attempt(ctx.info().attempt);
        self.scoring
            .score_run(run_id, last_attempt)
            .await
            .map_err(failure)
    }
}

/// Whether attempt `attempt` (counted from 1) of scoring is its last: the
/// workflow allows [`SCORE_ATTEMPTS`]. Its `SCORE_TOTAL` must leave room for
/// all of them, or Temporal stops the activity before its last attempt and
/// this never holds.
fn is_last_attempt(attempt: u32) -> bool {
    attempt >= SCORE_ATTEMPTS
}

#[cfg(test)]
mod test {
    use super::is_last_attempt;

    #[test]
    fn only_the_third_attempt_is_the_last() {
        assert!(!is_last_attempt(1));
        assert!(!is_last_attempt(2));
        assert!(is_last_attempt(3));
    }
}
