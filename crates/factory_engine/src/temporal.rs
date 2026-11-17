//! What the worker (`crate::inbound::temporal`) and the client that starts,
//! answers and cancels Runs (`crate::outbound::temporal_run_workflows`) and
//! starts the watches of base branches (`crate::outbound::temporal_main_watches`)
//! agree on: the task queue, and the workflows' and the update's names and
//! types.
//! Renaming any of them strands the Runs in flight.

use factory_records::domain::models::RunOutcome;
use temporalio_common::{
    HasWorkflowDefinition, SignalDefinition, UpdateDefinition, WorkflowDefinition,
};

use crate::domain::gate::AnswerGate;
use crate::domain::main_hold::MainWatch;
use crate::domain::models::RunStart;

/// The task queue the factory's workflow and activities run on.
pub const TASK_QUEUE: &str = "factory";

/// The workflow that walks a Run's Flow.
pub const FLOW_WORKFLOW: &str = "factory_flow";

/// The update that gives a person's answer to the Gate a Run waits on.
pub const ANSWER_GATE_UPDATE: &str = "factory_answer_gate";

/// The workflow that watches a base branch Runs merge to.
pub const MAIN_WATCH_WORKFLOW: &str = "factory_main_watch";

/// The signal that asks a base branch's watch to look again.
pub const LOOK_AGAIN_SIGNAL: &str = "factory_look_again";

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

/// The workflow that watches a base branch, as its starter sees it.
pub struct MainWatchWorkflowDefinition;

impl WorkflowDefinition for MainWatchWorkflowDefinition {
    type Input = MainWatch;
    type Output = ();

    fn name(&self) -> &str {
        MAIN_WATCH_WORKFLOW
    }
}

impl HasWorkflowDefinition for MainWatchWorkflowDefinition {
    type Run = Self;
}

/// The signal that asks a base branch's watch to look again, as its sender
/// sees it.
pub struct LookAgainSignal;

impl SignalDefinition for LookAgainSignal {
    type Workflow = MainWatchWorkflowDefinition;
    type Input = ();

    fn name(&self) -> &str {
        LOOK_AGAIN_SIGNAL
    }
}
