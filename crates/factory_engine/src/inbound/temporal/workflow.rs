//! `FlowWorkflow`: the one workflow that walks any Flow.
//!
//! It asks the interpreter for the next Step, runs it as an activity, and
//! hands the interpreter the Step's outcome, until the Run ends; then it
//! records the end. The interpreter is pure, so a replay walks the Flow the
//! same way from the recorded outcomes.

use std::time::Duration;

use factory_records::domain::models::{Outcome, RunOutcome};
use temporalio_common::RetryPolicy;
use temporalio_common::error::ApplicationFailure;
use temporalio_macros::{workflow, workflow_methods};
use temporalio_sdk::{ActivityOptions, WorkflowContext, WorkflowResult};

use crate::domain::interpreter::{Interpreter, NextStep};
use crate::domain::models::{RunFinish, RunStart, StepRequest};
use crate::inbound::temporal::activities::FactoryActivities;
use crate::temporal::FLOW_WORKFLOW;

/// How long one attempt of a Step may take.
const STEP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How many times a Step is tried before it ends `failed`.
const STEP_ATTEMPTS: u32 = 3;

/// How long recording a Run's end may take.
const RECORD_TIMEOUT: Duration = Duration::from_secs(60);

/// The workflow of one Run.
#[workflow]
#[derive(Default)]
pub struct FlowWorkflow;

#[workflow_methods]
impl FlowWorkflow {
    /// Walk `start`'s Flow to its end, and return how the Run ended.
    #[run(name = FLOW_WORKFLOW)]
    pub async fn run(
        ctx: &mut WorkflowContext<Self>,
        start: RunStart,
    ) -> WorkflowResult<RunOutcome> {
        let interpreter = Interpreter::new(&start.flow);
        let mut state = interpreter.start();
        let step_options = ActivityOptions::with_start_to_close_timeout(STEP_TIMEOUT)
            .retry_policy(
                RetryPolicy::builder()
                    .maximum_attempts(STEP_ATTEMPTS)
                    .build(),
            )
            .build();

        // A Flow that cannot go on still records the Run's end, then fails
        // the workflow with why, for Temporal to show.
        let mut stopped = None;
        let outcome = loop {
            let next = match interpreter.next_step(&mut state) {
                Ok(next) => next,
                Err(error) => {
                    stopped = Some(error);
                    break RunOutcome::Failed;
                }
            };
            let (entry, visit) = match next {
                NextStep::End(outcome) => break outcome,
                NextStep::Run { entry, visit } => (entry, visit),
            };

            let request = StepRequest {
                run_id: start.run_id,
                entry: entry.id.clone(),
                step: entry.step.clone(),
                visit,
                request: start.request.clone(),
            };
            // A Step that still fails after its retries ends `failed`, which
            // the Flow may route like any other outcome.
            let outcome = match ctx
                .execute_activity(FactoryActivities::run_step, request, step_options.clone())
                .await
            {
                Ok(report) => report.outcome,
                Err(_) => Outcome::failed(),
            };
            if let Err(error) = interpreter.record(&mut state, &outcome) {
                stopped = Some(error);
                break RunOutcome::Failed;
            }
        };

        let finish = RunFinish {
            run_id: start.run_id,
            outcome,
        };
        ctx.execute_activity(
            FactoryActivities::end_run,
            finish,
            ActivityOptions::start_to_close_timeout(RECORD_TIMEOUT),
        )
        .await?;

        match stopped {
            Some(error) => Err(ApplicationFailure::non_retryable(error).into()),
            None => Ok(outcome),
        }
    }
}
