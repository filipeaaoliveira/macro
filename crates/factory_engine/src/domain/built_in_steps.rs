//! The Steps the factory knows: `reply` acknowledges the Run's request, and
//! `plan` and `write` hand it to an agent.

#[cfg(test)]
mod test;

use factory_records::domain::models::Outcome;

use crate::domain::flow::StepName;
use crate::domain::models::{AgentTask, StepInput, StepReport};
use crate::domain::ports::{StepAgent, StepExecutor};

/// What the agent is told for a `plan` Step.
const PLAN_INSTRUCTIONS: &str = include_str!("built_in_steps/plan.md");

/// What the agent is told for a `write` Step.
const WRITE_INSTRUCTIONS: &str = include_str!("built_in_steps/write.md");

/// One of the Steps [`BuiltInSteps`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuiltInStep {
    /// Acknowledges the request, to check the engine end to end.
    Reply,
    /// An agent plans the change the request asks for.
    Plan,
    /// An agent writes the change its plan describes.
    Write,
}

impl BuiltInStep {
    fn named(step: &StepName) -> Option<Self> {
        match step.as_str() {
            "reply" => Some(Self::Reply),
            "plan" => Some(Self::Plan),
            "write" => Some(Self::Write),
            _ => None,
        }
    }
}

/// Why a built-in Step's work failed.
#[derive(Debug, thiserror::Error)]
pub enum BuiltInStepError {
    /// No built-in Step has this name.
    #[error("no built-in step is named {0}")]
    UnknownStep(StepName),
    /// The agent could not do the Step.
    #[error("the agent failed: {0:?}")]
    Agent(rootcause::Report),
    /// The agent ended the Step with an outcome the Step does not have.
    #[error("the agent ended step {step} with {outcome}, which it does not end with")]
    UnexpectedOutcome {
        /// The Step.
        step: StepName,
        /// The outcome the agent handed back.
        outcome: Outcome,
    },
}

/// The built-in Steps, over the agent that does `plan` and `write`.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInSteps<A> {
    agent: A,
}

impl<A: StepAgent> BuiltInSteps<A> {
    /// The built-in Steps, with `agent` doing the agent Steps.
    pub fn new(agent: A) -> Self {
        Self { agent }
    }

    /// Hand `step` to the agent with `instructions`, and check it ends with
    /// one of `outcomes`.
    async fn ask_agent(
        &self,
        step: &StepName,
        instructions: &str,
        outcomes: Vec<Outcome>,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        let task = AgentTask {
            run_id: input.run_id,
            step: step.clone(),
            prompt: prompt(instructions, &outcomes, input),
            outcomes: outcomes.clone(),
        };
        let answer = self.agent.answer(task).await.map_err(|error| {
            BuiltInStepError::Agent(rootcause::Report::new(error).into_dynamic())
        })?;

        if !outcomes.contains(&answer.outcome) {
            return Err(BuiltInStepError::UnexpectedOutcome {
                step: step.clone(),
                outcome: answer.outcome,
            });
        }
        Ok(StepReport {
            outcome: answer.outcome,
            output: answer.text,
        })
    }
}

impl<A: StepAgent> StepExecutor for BuiltInSteps<A> {
    type Err = BuiltInStepError;

    fn serves(&self, step: &StepName) -> bool {
        BuiltInStep::named(step).is_some()
    }

    async fn run(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        let built_in =
            BuiltInStep::named(step).ok_or_else(|| BuiltInStepError::UnknownStep(step.clone()))?;
        match built_in {
            BuiltInStep::Reply => Ok(StepReport {
                outcome: Outcome::done(),
                output: format!("Received the request: {}", input.request),
            }),
            BuiltInStep::Plan => {
                self.ask_agent(step, PLAN_INSTRUCTIONS, vec![Outcome::done()], input)
                    .await
            }
            BuiltInStep::Write => {
                let outcomes = vec![Outcome::done(), Outcome::unchanged()];
                self.ask_agent(step, WRITE_INSTRUCTIONS, outcomes, input)
                    .await
            }
        }
    }
}

/// The agent's prompt: the Step's instructions, the outcomes it may end
/// with, the Run's request, and what each earlier Step produced, oldest
/// first, so a later Step reads the plan it carries out.
fn prompt(instructions: &str, outcomes: &[Outcome], input: StepInput<'_>) -> String {
    let outcomes = outcomes
        .iter()
        .map(|outcome| format!("`{outcome}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut prompt = format!(
        "{instructions}\nEnd the step with one of these outcomes: {outcomes}.\n\n\
         ## Request\n\n{}\n",
        input.request
    );

    for step in input.earlier {
        let Some(end) = &step.end else {
            continue;
        };
        prompt.push_str(&format!(
            "\n## Step {}, visit {}: {}\n\n{}\n",
            step.entry, step.visit, end.outcome, end.output
        ));
    }
    prompt
}
