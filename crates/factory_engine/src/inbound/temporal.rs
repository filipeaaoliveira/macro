//! The Temporal worker that runs the Runs' workflows and their Steps.

mod activities;
mod workflow;

use std::time::Duration;

use temporalio_client::Client;
use temporalio_sdk::{Runtime, Worker, WorkerOptions};

use crate::domain::ports::StepService;
use crate::inbound::temporal::activities::FactoryActivities;
use crate::inbound::temporal::workflow::FlowWorkflow;
use crate::temporal::TASK_QUEUE;

/// How long a shutting-down worker lets running Steps go on before it
/// cancels them. A cancelled Step drops its work and fails as the worker's
/// shutdown, which Temporal retries on another worker at once, in the same
/// agent session, rather than after the Step's heartbeat timeout.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// A worker on the factory's task queue that runs the Flow workflow and its
/// activities, over `steps`.
pub fn worker(
    runtime: &Runtime,
    client: Client,
    steps: impl StepService,
) -> Result<Worker, rootcause::Report> {
    let options = WorkerOptions::new(TASK_QUEUE)
        .register_workflow::<FlowWorkflow>()?
        .register_activities(FactoryActivities::new(steps))
        .graceful_shutdown_period(SHUTDOWN_GRACE)
        .build();
    Ok(Worker::new(runtime, client, options)?)
}
