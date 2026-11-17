//! `MainWatchWorkflow`: the watch of a base branch Runs merge to, one per
//! base, which sets and clears its Main Hold.
//!
//! A merge starts it, and so does a `merge` that finds its base held, when
//! none runs. It looks at the checks on the base's head at once, then every
//! [`LOOK_EVERY`]: a red look holds the base and files the task with the
//! suspect merges, once per red period, and a green look clears the hold
//! and ends the watch. While nothing holds the base and its checks have not
//! finished, it gives up after [`UNHELD_LOOKS`] looks; a held base is
//! watched until it is green, across continue-as-new. Each look is one
//! activity: what the watch decides comes only from their results.

use std::sync::Arc;
use std::time::Duration;

use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::{
    ActivityCloseTimeouts, ActivityOptions, ContinueAsNewOptions, WorkflowContext, WorkflowResult,
    WorkflowTermination,
};

use crate::domain::main_hold::{MainLook, MainState, MainWatch};
use crate::domain::models::StepError;
use crate::domain::ports::MainWatchService;
use crate::inbound::temporal::activities::failure;
use crate::temporal::MAIN_WATCH_WORKFLOW;

/// How long the watch waits between two looks; a second in the tests, which
/// run it on a real Temporal.
#[cfg(not(test))]
const LOOK_EVERY: Duration = Duration::from_secs(3 * 60);
#[cfg(test)]
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// How many looks in a row that hold nothing the watch takes before it
/// ends: about an hour of checks that do not finish, or of looks that fail.
const UNHELD_LOOKS: u32 = 20;

/// How long one look may take.
const LOOK_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// How long a look may take in all, across its attempts.
const LOOK_TOTAL: Duration = Duration::from_secs(10 * 60);

/// The watch of one base branch.
#[workflow]
#[derive(Default)]
pub struct MainWatchWorkflow;

#[workflow_methods]
impl MainWatchWorkflow {
    /// Watch `watch`'s base until its checks pass, or until it holds
    /// nothing for [`UNHELD_LOOKS`] looks.
    #[run(name = MAIN_WATCH_WORKFLOW)]
    pub async fn run(ctx: &mut WorkflowContext<Self>, watch: MainWatch) -> WorkflowResult<()> {
        let options =
            ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::ScheduleAndStartToClose {
                schedule_to_close: LOOK_TOTAL,
                start_to_close: LOOK_TIMEOUT,
            })
            .build();
        let mut unheld = 0;
        loop {
            let looked = ctx
                .execute_activity(
                    MainWatchActivities::look_at_main,
                    watch.clone(),
                    options.clone(),
                )
                .await;
            if ctx.cancellation_token().is_cancelled() {
                return Err(WorkflowTermination::cancelled());
            }
            match looked {
                Ok(MainLook {
                    state: MainState::Green | MainState::Gone,
                    ..
                }) => return Ok(()),
                Ok(MainLook { held: true, .. }) => unheld = 0,
                // Checks that do not finish, or a look that failed, as with
                // an owner whose GitHub link no longer works: a held base
                // is looked at again later, when a held merge restarts the
                // watch as its own owner.
                Ok(MainLook { held: false, .. }) | Err(_) => {
                    unheld += 1;
                    if unheld >= UNHELD_LOOKS {
                        return Ok(());
                    }
                }
            }

            let _ = ctx.timer(LOOK_EVERY).await;
            if ctx.cancellation_token().is_cancelled() {
                return Err(WorkflowTermination::cancelled());
            }
            if ctx.continue_as_new_suggested() {
                return ctx
                    .continue_as_new(watch, ContinueAsNewOptions::default())
                    .map(|never| match never {});
            }
        }
    }
}

/// The look as a trait object: `#[activities]` does not take a generic
/// type.
#[async_trait::async_trait]
trait DynMainWatchService: Send + Sync {
    async fn look(&self, watch: MainWatch) -> Result<MainLook, StepError>;
}

#[async_trait::async_trait]
impl<T: MainWatchService> DynMainWatchService for T {
    async fn look(&self, watch: MainWatch) -> Result<MainLook, StepError> {
        MainWatchService::look(self, watch).await
    }
}

/// The watch's activities.
pub struct MainWatchActivities {
    service: Arc<dyn DynMainWatchService>,
}

impl MainWatchActivities {
    /// The activities, over `service`.
    pub fn new(service: impl MainWatchService) -> Self {
        Self {
            service: Arc::new(service),
        }
    }
}

#[activities]
impl MainWatchActivities {
    /// Look at the checks on the head of a base branch once, and hold or
    /// clear it.
    #[activity(name = "factory_look_at_main")]
    pub async fn look_at_main(
        self: Arc<Self>,
        _ctx: ActivityContext,
        watch: MainWatch,
    ) -> Result<MainLook, ActivityError> {
        self.service.look(watch).await.map_err(failure)
    }
}
