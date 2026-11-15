//! Temporal implementation of the [`RunScheduler`] port.

use temporalio_client::errors::WorkflowStartError;
use temporalio_client::{Client, WorkflowStartOptions};

use crate::domain::models::RunStart;
use crate::domain::ports::RunScheduler;
use crate::temporal::{FlowWorkflowDefinition, TASK_QUEUE};

/// Starts each Run's workflow on the factory's task queue, with the Run's id
/// as its workflow id.
#[derive(Clone)]
pub struct TemporalRunScheduler {
    client: Client,
}

impl TemporalRunScheduler {
    /// A scheduler over a connected Temporal client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

/// Errors starting a Run's workflow.
#[derive(Debug, thiserror::Error)]
pub enum TemporalRunSchedulerErr {
    /// Temporal refused or did not answer the start.
    #[error(transparent)]
    Start(#[from] WorkflowStartError),
}

impl RunScheduler for TemporalRunScheduler {
    type Err = TemporalRunSchedulerErr;

    #[tracing::instrument(err, skip_all, fields(run_id = %start.run_id))]
    async fn start(&self, start: &RunStart) -> Result<(), Self::Err> {
        let options = WorkflowStartOptions::new(TASK_QUEUE, start.run_id.to_string()).build();
        let started = self
            .client
            .start_workflow(FlowWorkflowDefinition, start.clone(), options)
            .await;
        match started {
            Ok(_) | Err(WorkflowStartError::AlreadyStarted { .. }) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
