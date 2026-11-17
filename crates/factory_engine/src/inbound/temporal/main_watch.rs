//! `MainWatchWorkflow`: the watch of a base branch Runs merge to, one per
//! base, which sets and clears its Main Hold.
//!
//! Each merge signals it to look again (`factory_look_again`), starting it
//! when none runs, and so does a `merge` that finds its base held. It looks
//! at the checks the base requires on its head at once, then every
//! [`LOOK_EVERY`]: a red look holds the base and files the task with the
//! suspect merges, once per red period, and a green look clears the hold.
//! It ends only when a look says it may ([`MainLook::ends_watch`]): never
//! while the base is held or a look fails, and, while checks run with
//! nothing held, once the base's latest merge is hours old. A signal that
//! arrives during the last look makes it look again rather than end; one
//! that arrives as it ends fails that end, so Temporal replays the watch
//! with the signal. It continues as new every [`LOOKS_PER_RUN`] looks. Each
//! look is one activity: what the watch decides comes only from their
//! results and the signals.

use std::sync::Arc;
use std::time::Duration;

use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::activities::{ActivityContext, ActivityError};
use temporalio_sdk::{
    ActivityCloseTimeouts, ActivityOptions, ContinueAsNewOptions, SyncWorkflowContext,
    WorkflowContext, WorkflowResult, WorkflowTermination,
};

use factory_records::domain::models::RunId;

use crate::domain::main_hold::{MainLook, MainWatch};
use crate::domain::models::StepError;
use crate::domain::ports::MainWatchService;
use crate::inbound::temporal::activities::failure;
use crate::temporal::{LOOK_AGAIN_SIGNAL, MAIN_WATCH_WORKFLOW};

/// How long the watch waits between two looks; a second in the tests, which
/// run it on a real Temporal.
#[cfg(not(test))]
const LOOK_EVERY: Duration = Duration::from_secs(3 * 60);
#[cfg(test)]
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// How many looks one run of the watch takes before it continues as new,
/// which keeps its history short; three in the tests.
#[cfg(not(test))]
const LOOKS_PER_RUN: u32 = 200;
#[cfg(test)]
const LOOKS_PER_RUN: u32 = 3;

/// How long one look may take.
const LOOK_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// How long a look may take in all, across its attempts.
const LOOK_TOTAL: Duration = Duration::from_secs(10 * 60);

/// The watch of one base branch.
#[workflow]
#[derive(Default)]
pub struct MainWatchWorkflow {
    /// Whether a merge asked for a look since the last one started.
    again: bool,
    /// The Run that last asked for a look, as a held merge does: as its
    /// owner, among others, the watch reads GitHub.
    asked_by: Option<RunId>,
}

#[workflow_methods]
impl MainWatchWorkflow {
    /// Watch `watch`'s base until a look says the watch may end.
    #[run(name = MAIN_WATCH_WORKFLOW)]
    pub async fn run(ctx: &mut WorkflowContext<Self>, mut watch: MainWatch) -> WorkflowResult<()> {
        let options =
            ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::ScheduleAndStartToClose {
                schedule_to_close: LOOK_TOTAL,
                start_to_close: LOOK_TIMEOUT,
            })
            .build();
        let mut looks = 0;
        loop {
            // A look asked for from here on is one this look may not cover.
            let asked_by = ctx.state_mut(|workflow| {
                workflow.again = false;
                workflow.asked_by.take()
            });
            if let Some(run_id) = asked_by {
                watch.run_id = run_id;
            }
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
            looks += 1;

            // A look that failed decides nothing: the base may be held.
            let may_end = looked.as_ref().is_ok_and(MainLook::ends_watch);
            if may_end {
                if !ctx.state(|workflow| workflow.again) {
                    return Ok(());
                }
                continue;
            }

            let _ = ctx.timer(LOOK_EVERY).await;
            if ctx.cancellation_token().is_cancelled() {
                return Err(WorkflowTermination::cancelled());
            }
            if looks >= LOOKS_PER_RUN || ctx.continue_as_new_suggested() {
                return ctx
                    .continue_as_new(watch, ContinueAsNewOptions::default())
                    .map(|never| match never {});
            }
        }
    }

    /// Ask the watch to look again, for the Run `run_id`: a merge to its
    /// base landed, or waits on its hold.
    #[signal(name = LOOK_AGAIN_SIGNAL)]
    pub fn look_again(&mut self, _ctx: &mut SyncWorkflowContext<Self>, run_id: RunId) {
        self.again = true;
        self.asked_by = Some(run_id);
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
