//! Temporal implementation of the [`MainWatches`] port.

use temporalio_client::errors::WorkflowStartError;
use temporalio_client::{Client, WorkflowIdConflictPolicy, WorkflowStartOptions};

use crate::domain::main_hold::MainWatch;
use crate::domain::ports::{MainWatches, Retryable, RunWorkflowsError};
use crate::outbound::temporal_run_workflows::{from_status, other};
use crate::temporal::{MainWatchWorkflowDefinition, TASK_QUEUE};

/// Starts the watch of a base branch on the factory's task queue, one per
/// base: its workflow id names the base, and a start while one runs uses
/// the running one.
#[derive(Clone)]
pub struct TemporalMainWatches {
    client: Client,
    task_queue: String,
}

impl TemporalMainWatches {
    /// The watches, over a connected Temporal client.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            task_queue: TASK_QUEUE.to_string(),
        }
    }

    /// The watches on `task_queue`: a test's own, which the stack's worker
    /// never takes.
    #[cfg(test)]
    pub(crate) fn on(client: Client, task_queue: &str) -> Self {
        Self {
            client,
            task_queue: task_queue.to_string(),
        }
    }
}

/// Temporal's outage passes; anything else is ours to look at.
impl Retryable for RunWorkflowsError {
    fn retryable(&self) -> bool {
        matches!(self, RunWorkflowsError::Unavailable(_))
    }
}

impl MainWatches for TemporalMainWatches {
    type Err = RunWorkflowsError;

    #[tracing::instrument(err, skip_all, fields(repo_url = %watch.repo_url, base = %watch.base_branch))]
    async fn watch(&self, watch: &MainWatch) -> Result<(), RunWorkflowsError> {
        let options = WorkflowStartOptions::new(self.task_queue.clone(), watch.workflow_id())
            .id_conflict_policy(WorkflowIdConflictPolicy::UseExisting)
            .build();
        let started = self
            .client
            .start_workflow(MainWatchWorkflowDefinition, watch.clone(), options)
            .await;
        match started {
            Ok(_) | Err(WorkflowStartError::AlreadyStarted { .. }) => Ok(()),
            Err(WorkflowStartError::Rpc(status)) => Err(from_status(status)),
            Err(error) => Err(other(error)),
        }
    }
}
