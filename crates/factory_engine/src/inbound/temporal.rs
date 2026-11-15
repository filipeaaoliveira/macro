//! The Temporal worker that runs the Runs' workflows and their Steps.

mod activities;
mod workflow;

use temporalio_client::Client;
use temporalio_sdk::{Runtime, Worker, WorkerOptions};

use crate::domain::ports::StepService;
use crate::inbound::temporal::activities::FactoryActivities;
use crate::inbound::temporal::workflow::FlowWorkflow;
use crate::temporal::TASK_QUEUE;

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
        .build();
    Ok(Worker::new(runtime, client, options)?)
}
