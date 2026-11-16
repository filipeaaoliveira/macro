//! Temporal implementation of the [`RunWorkflows`] port.

use factory_records::domain::models::RunId;
use temporalio_client::errors::{
    WorkflowInteractionError, WorkflowStartError, WorkflowUpdateError,
};
use temporalio_client::{
    Client, WorkflowCancelOptions, WorkflowExecuteUpdateOptions, WorkflowStartOptions,
};

use crate::domain::gate::AnswerGate;
use crate::domain::models::RunStart;
use crate::domain::ports::{AnswerDelivery, RunWorkflows};
use crate::temporal::{AnswerGateUpdate, FlowWorkflowDefinition, TASK_QUEUE};

/// Why a Run's workflow is cancelled, for Temporal to show.
const CANCEL_REASON: &str = "cancelled through the Run API";

/// Starts each Run's workflow on the factory's task queue, with the Run's id
/// as its workflow id, and sends it answers and cancellations.
#[derive(Clone)]
pub struct TemporalRunWorkflows {
    client: Client,
}

impl TemporalRunWorkflows {
    /// The Run workflows, over a connected Temporal client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

/// Errors reaching a Run's workflow.
#[derive(Debug, thiserror::Error)]
pub enum TemporalRunWorkflowsErr {
    /// Temporal refused or did not answer the start.
    #[error(transparent)]
    Start(#[from] WorkflowStartError),
    /// Temporal did not deliver the answer.
    #[error(transparent)]
    Update(#[from] WorkflowUpdateError),
    /// Temporal did not take the cancellation.
    #[error(transparent)]
    Cancel(#[from] WorkflowInteractionError),
}

impl RunWorkflows for TemporalRunWorkflows {
    type Err = TemporalRunWorkflowsErr;

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

    #[tracing::instrument(err, skip_all, fields(run_id = %run_id))]
    async fn answer(
        &self,
        run_id: RunId,
        answer: &AnswerGate,
    ) -> Result<AnswerDelivery, Self::Err> {
        let handle = self
            .client
            .get_workflow_handle::<FlowWorkflowDefinition>(run_id.to_string());
        let sent = handle
            .execute_update(
                AnswerGateUpdate,
                answer.clone(),
                WorkflowExecuteUpdateOptions::default(),
            )
            .await;
        match sent {
            Ok(()) => Ok(AnswerDelivery::Taken),
            // The workflow's validator refused the answer.
            Err(WorkflowUpdateError::Failed(failure)) => {
                Ok(AnswerDelivery::Refused(failure.message))
            }
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %run_id))]
    async fn cancel(&self, run_id: RunId) -> Result<(), Self::Err> {
        let handle = self
            .client
            .get_workflow_handle::<FlowWorkflowDefinition>(run_id.to_string());
        let options = WorkflowCancelOptions::builder()
            .reason(CANCEL_REASON.to_string())
            .build();
        match handle.cancel(options).await {
            // A workflow that already ended has nothing left to cancel.
            Ok(()) | Err(WorkflowInteractionError::NotFound(_)) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
