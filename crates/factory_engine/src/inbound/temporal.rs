//! The Temporal worker that runs the Runs' workflows and their Steps.

mod activities;
mod main_watch;
#[cfg(test)]
mod test;
mod workflow;

use std::time::Duration;

use temporalio_client::Client;
use temporalio_sdk::{Runtime, Worker, WorkerOptions};

use crate::domain::ports::{MainWatchService, StepService};
use crate::inbound::temporal::activities::FactoryActivities;
use crate::inbound::temporal::main_watch::{MainWatchActivities, MainWatchWorkflow};
use crate::inbound::temporal::workflow::FlowWorkflow;
use crate::temporal::TASK_QUEUE;

/// How long a shutting-down worker lets running Steps go on before it
/// cancels them. A cancelled Step drops its work and fails as the worker's
/// shutdown, which Temporal retries on another worker at once, in the same
/// agent session, rather than after the Step's heartbeat timeout.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// The longest a worker holds back a running activity's heartbeat. A Run's
/// cancel reaches a working Step only on the answer to one, so within about
/// this; the SDK would otherwise wait up to 80% of the heartbeat timeout.
const MAX_HEARTBEAT_THROTTLE: Duration = Duration::from_secs(4);

/// A worker on the factory's task queue that runs the Flow workflow and its
/// activities, over `steps`, and the watches of base branches, over
/// `watches`.
pub fn worker(
    runtime: &Runtime,
    client: Client,
    steps: impl StepService,
    watches: impl MainWatchService,
) -> Result<Worker, rootcause::Report> {
    worker_on(TASK_QUEUE, runtime, client, steps, watches)
}

/// A worker on `task_queue`, as [`worker`] is on the factory's.
fn worker_on(
    task_queue: &str,
    runtime: &Runtime,
    client: Client,
    steps: impl StepService,
    watches: impl MainWatchService,
) -> Result<Worker, rootcause::Report> {
    let options = WorkerOptions::new(task_queue)
        .register_workflow::<FlowWorkflow>()?
        .register_workflow::<MainWatchWorkflow>()?
        .register_activities(FactoryActivities::new(steps))
        .register_activities(MainWatchActivities::new(watches))
        .graceful_shutdown_period(SHUTDOWN_GRACE)
        .max_heartbeat_throttle_interval(MAX_HEARTBEAT_THROTTLE)
        .build();
    Ok(Worker::new(runtime, client, options)?)
}
