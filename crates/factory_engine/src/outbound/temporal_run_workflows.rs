//! Temporal implementation of the [`RunWorkflows`] port.

#[cfg(test)]
mod test;

use factory_records::domain::models::RunId;
use temporalio_client::errors::{
    WorkflowInteractionError, WorkflowStartError, WorkflowUpdateError,
};
use temporalio_client::tonic::{Code, Status};
use temporalio_client::{
    Client, WorkflowCancelOptions, WorkflowDescribeOptions, WorkflowExecuteUpdateOptions,
    WorkflowStartOptions,
};
use upstream_error::{UpstreamError, UpstreamFailure};

use crate::domain::gate::AnswerGate;
use crate::domain::models::RunStart;
use crate::domain::ports::{AnswerDelivery, RunWorkflows, RunWorkflowsError};
use crate::temporal::{AnswerGateUpdate, FlowWorkflowDefinition, TASK_QUEUE};

/// Why a Run's workflow is cancelled, for Temporal to show.
const CANCEL_REASON: &str = "cancelled through the Run API";

/// Temporal, named as an error shown to the API's caller names it.
const TEMPORAL: &str = "The workflow service";

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

/// A gRPC failure: Temporal's outage when retrying later can help, anything
/// else ours to look at.
fn from_status(status: Status) -> RunWorkflowsError {
    match status.code() {
        Code::Unavailable | Code::DeadlineExceeded | Code::ResourceExhausted | Code::Aborted => {
            RunWorkflowsError::Unavailable(UpstreamError {
                service: TEMPORAL,
                failure: UpstreamFailure::Unavailable,
            })
        }
        _ => RunWorkflowsError::Other(rootcause::report!("{status}")),
    }
}

fn other(error: impl std::error::Error + Send + Sync + 'static) -> RunWorkflowsError {
    RunWorkflowsError::Other(rootcause::Report::new(error).into_dynamic())
}

impl RunWorkflows for TemporalRunWorkflows {
    #[tracing::instrument(err, skip_all, fields(run_id = %start.run_id))]
    async fn start(&self, start: &RunStart) -> Result<(), RunWorkflowsError> {
        let options = WorkflowStartOptions::new(TASK_QUEUE, start.run_id.to_string()).build();
        let started = self
            .client
            .start_workflow(FlowWorkflowDefinition, start.clone(), options)
            .await;
        match started {
            Ok(_) | Err(WorkflowStartError::AlreadyStarted { .. }) => Ok(()),
            Err(WorkflowStartError::Rpc(status)) => Err(from_status(status)),
            Err(error) => Err(other(error)),
        }
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %run_id))]
    async fn exists(&self, run_id: RunId) -> Result<bool, RunWorkflowsError> {
        let handle = self
            .client
            .get_workflow_handle::<FlowWorkflowDefinition>(run_id.to_string());
        match handle.describe(WorkflowDescribeOptions::default()).await {
            Ok(_) => Ok(true),
            Err(WorkflowInteractionError::NotFound(_)) => Ok(false),
            Err(WorkflowInteractionError::Rpc(status)) => Err(from_status(status)),
            Err(error) => Err(other(error)),
        }
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %run_id))]
    async fn answer(
        &self,
        run_id: RunId,
        answer: &AnswerGate,
    ) -> Result<AnswerDelivery, RunWorkflowsError> {
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
            Err(WorkflowUpdateError::Rpc(status)) => Err(from_status(status)),
            Err(error) => Err(other(error)),
        }
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %run_id))]
    async fn cancel(&self, run_id: RunId) -> Result<(), RunWorkflowsError> {
        let handle = self
            .client
            .get_workflow_handle::<FlowWorkflowDefinition>(run_id.to_string());
        let options = WorkflowCancelOptions::builder()
            .reason(CANCEL_REASON.to_string())
            .build();
        match handle.cancel(options).await {
            // A workflow that already ended has nothing left to cancel.
            Ok(()) | Err(WorkflowInteractionError::NotFound(_)) => Ok(()),
            Err(WorkflowInteractionError::Rpc(status)) => Err(from_status(status)),
            Err(error) => Err(other(error)),
        }
    }
}
