//! The activity a Run's workflow calls once the Run ended, over the scoring
//! service. The judges' model calls run here, and what they decide is only
//! recorded, never read back by the workflow.

use std::sync::Arc;

use factory_records::domain::models::RunId;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

use super::activities::failure;
use crate::domain::models::StepError;
use crate::domain::ports::ScoringService;

/// The scoring service as a trait object: `#[activities]` takes no generic
/// type, as for the Step service.
#[async_trait::async_trait]
trait DynScoringService: Send + Sync {
    async fn score_run(&self, run_id: RunId) -> Result<(), StepError>;
}

#[async_trait::async_trait]
impl<T: ScoringService> DynScoringService for T {
    async fn score_run(&self, run_id: RunId) -> Result<(), StepError> {
        ScoringService::score_run(self, run_id).await
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
        _ctx: ActivityContext,
        run_id: RunId,
    ) -> Result<(), ActivityError> {
        self.scoring.score_run(run_id).await.map_err(failure)
    }
}
