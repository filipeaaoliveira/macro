//! The Temporal worker that runs the Runs' workflows and their Steps.

mod activities;
mod gate_activities;
mod main_watch;
mod scoring_activities;
#[cfg(test)]
mod test;
mod workflow;

use std::time::Duration;

use temporalio_client::Client;
use temporalio_sdk::{Runtime, Worker, WorkerOptions};

use crate::domain::ports::{
    GateReplyService, MainWatchService, ReviewService, ScoringService, StepService,
};
use crate::inbound::temporal::activities::FactoryActivities;
use crate::inbound::temporal::gate_activities::GateActivities;
use crate::inbound::temporal::main_watch::{MainWatchActivities, MainWatchWorkflow};
use crate::inbound::temporal::scoring_activities::ScoringActivities;
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

/// What a worker runs its activities over.
pub struct WorkerServices<S, R, G, M, C> {
    /// The Steps, and the records of Steps, Gates and Runs.
    pub steps: S,
    /// The review panels' rounds.
    pub reviews: R,
    /// Gate replies and the stand-in.
    pub gates: G,
    /// The watches of base branches.
    pub watches: M,
    /// The Scorers' judgments and Benchmark trials' results of Runs that
    /// ended.
    pub scoring: C,
}

/// A worker on the factory's task queue that runs the Flow workflow and its
/// activities, and the watches of base branches, over `services`.
pub fn worker(
    runtime: &Runtime,
    client: Client,
    services: WorkerServices<
        impl StepService,
        impl ReviewService,
        impl GateReplyService,
        impl MainWatchService,
        impl ScoringService,
    >,
) -> Result<Worker, rootcause::Report> {
    worker_on(TASK_QUEUE, runtime, client, services)
}

/// A worker on `task_queue`, as [`worker`] is on the factory's.
fn worker_on(
    task_queue: &str,
    runtime: &Runtime,
    client: Client,
    services: WorkerServices<
        impl StepService,
        impl ReviewService,
        impl GateReplyService,
        impl MainWatchService,
        impl ScoringService,
    >,
) -> Result<Worker, rootcause::Report> {
    let WorkerServices {
        steps,
        reviews,
        gates,
        watches,
        scoring,
    } = services;
    let options = WorkerOptions::new(task_queue)
        .register_workflow::<FlowWorkflow>()?
        .register_workflow::<MainWatchWorkflow>()?
        .register_activities(FactoryActivities::new(steps, reviews))
        .register_activities(GateActivities::new(gates))
        .register_activities(MainWatchActivities::new(watches))
        .register_activities(ScoringActivities::new(scoring))
        .graceful_shutdown_period(SHUTDOWN_GRACE)
        .max_heartbeat_throttle_interval(MAX_HEARTBEAT_THROTTLE)
        .build();
    Ok(Worker::new(runtime, client, options)?)
}
