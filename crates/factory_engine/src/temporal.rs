//! What the worker (`crate::inbound::temporal`) and the scheduler
//! (`crate::outbound::temporal_scheduler`) agree on: the task queue, and the
//! workflow's name and types. Renaming either strands the Runs in flight.

use factory_records::domain::models::RunOutcome;
use temporalio_common::{HasWorkflowDefinition, WorkflowDefinition};

use crate::domain::models::RunStart;

/// The task queue the factory's workflow and activities run on.
pub const TASK_QUEUE: &str = "factory";

/// The workflow that walks a Run's Flow.
pub const FLOW_WORKFLOW: &str = "factory_flow";

/// The workflow that walks a Run's Flow, as its starter sees it.
pub struct FlowWorkflowDefinition;

impl WorkflowDefinition for FlowWorkflowDefinition {
    type Input = RunStart;
    type Output = RunOutcome;

    fn name(&self) -> &str {
        FLOW_WORKFLOW
    }
}

impl HasWorkflowDefinition for FlowWorkflowDefinition {
    type Run = Self;
}
