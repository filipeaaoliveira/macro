//! What the worker (`crate::inbound::temporal`) and the client that starts,
//! answers and cancels Runs (`crate::outbound::temporal_run_workflows`) agree
//! on: the task queue, and the workflow's and its update's names and types.
//! Renaming any of them strands the Runs in flight.

use factory_records::domain::models::RunOutcome;
use temporalio_common::{HasWorkflowDefinition, UpdateDefinition, WorkflowDefinition};

use crate::domain::gate::AnswerGate;
use crate::domain::models::RunStart;

/// The task queue the factory's workflow and activities run on.
pub const TASK_QUEUE: &str = "factory";

/// The workflow that walks a Run's Flow.
pub const FLOW_WORKFLOW: &str = "factory_flow";

/// The update that gives a person's answer to the Gate a Run waits on.
pub const ANSWER_GATE_UPDATE: &str = "factory_answer_gate";

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

/// The update that gives a person's answer to the Gate a Run waits on, as
/// its sender sees it. The workflow refuses an answer it does not take.
pub struct AnswerGateUpdate;

impl UpdateDefinition for AnswerGateUpdate {
    type Workflow = FlowWorkflowDefinition;
    type Input = AnswerGate;
    type Output = ();

    fn name(&self) -> &str {
        ANSWER_GATE_UPDATE
    }
}
