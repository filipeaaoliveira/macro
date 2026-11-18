//! The activities a Run's workflow calls for Gate replies and the stand-in,
//! over the Gate reply service. The model's calls run here, so a replay
//! reads their results from the history and never calls the model again.

use std::sync::Arc;

use factory_records::domain::models::Outcome;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

use super::activities::failure;
use crate::domain::gate::{ReplyRead, ReplyReading, StandInAsking, StandInKeeping, StandingIn};
use crate::domain::models::StepError;
use crate::domain::ports::GateReplyService;

/// The Gate reply service as a trait object: `#[activities]` takes no
/// generic type, as for the Step service.
#[async_trait::async_trait]
trait DynGateReplyService: Send + Sync {
    async fn read_reply(&self, reading: ReplyReading) -> Result<ReplyRead, StepError>;

    async fn stand_in(&self, asking: StandInAsking) -> Result<Option<StandingIn>, StepError>;

    async fn keep_stand_in(&self, keeping: StandInKeeping) -> Result<Option<Outcome>, StepError>;
}

#[async_trait::async_trait]
impl<T: GateReplyService> DynGateReplyService for T {
    async fn read_reply(&self, reading: ReplyReading) -> Result<ReplyRead, StepError> {
        GateReplyService::read_reply(self, reading).await
    }

    async fn stand_in(&self, asking: StandInAsking) -> Result<Option<StandingIn>, StepError> {
        GateReplyService::stand_in(self, asking).await
    }

    async fn keep_stand_in(&self, keeping: StandInKeeping) -> Result<Option<Outcome>, StepError> {
        GateReplyService::keep_stand_in(self, keeping).await
    }
}

/// The activities of Gate replies and the stand-in.
pub struct GateActivities {
    gates: Arc<dyn DynGateReplyService>,
}

impl GateActivities {
    /// The activities, over `gates`.
    pub fn new(gates: impl GateReplyService) -> Self {
        Self {
            gates: Arc::new(gates),
        }
    }
}

#[activities]
impl GateActivities {
    /// Read a person's words on a Gate: answer it with what they say, or
    /// ask back on it.
    #[activity(name = "factory_read_gate_reply")]
    pub async fn read_gate_reply(
        self: Arc<Self>,
        _ctx: ActivityContext,
        reading: ReplyReading,
    ) -> Result<ReplyRead, ActivityError> {
        self.gates.read_reply(reading).await.map_err(failure)
    }

    /// Ask the stand-in about a Gate no Standing Rule answered, and keep
    /// its answer, if it gives one.
    #[activity(name = "factory_stand_in_gate")]
    pub async fn stand_in_gate(
        self: Arc<Self>,
        _ctx: ActivityContext,
        asking: StandInAsking,
    ) -> Result<Option<StandingIn>, ActivityError> {
        self.gates.stand_in(asking).await.map_err(failure)
    }

    /// Answer a Gate with the stand-in's answer, once its owner let it
    /// stand, unless the Run now forbids it: then none, and the Gate is the
    /// owner's again.
    #[activity(name = "factory_keep_stand_in")]
    pub async fn keep_stand_in(
        self: Arc<Self>,
        _ctx: ActivityContext,
        keeping: StandInKeeping,
    ) -> Result<Option<Outcome>, ActivityError> {
        self.gates.keep_stand_in(keeping).await.map_err(failure)
    }
}
