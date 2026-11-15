//! The Steps the factory's worker does itself.

#[cfg(test)]
mod test;

use std::convert::Infallible;

use factory_records::domain::models::Outcome;

use crate::domain::flow::StepName;
use crate::domain::models::StepReport;
use crate::domain::ports::StepExecutor;

/// The `reply` Step: answers the Run's request and ends `done`.
const REPLY: &str = "reply";

/// The Steps that need nothing outside the worker. `reply` acknowledges the
/// request in its output; it is the Step a Flow runs to check the engine end
/// to end.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuiltInSteps;

impl StepExecutor for BuiltInSteps {
    type Err = Infallible;

    fn serves(&self, step: &StepName) -> bool {
        step.as_str() == REPLY
    }

    async fn run(&self, _step: &StepName, request: &str) -> Result<StepReport, Infallible> {
        Ok(StepReport {
            outcome: Outcome::done(),
            output: format!("Received the request: {request}"),
        })
    }
}
