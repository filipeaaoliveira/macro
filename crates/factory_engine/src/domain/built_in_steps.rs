//! The Steps the factory knows: `reply` acknowledges the Run's request, and
//! `plan` and `write` hand it to an agent.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, Step, StepKey};
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::flow::StepName;
use crate::domain::models::{AgentTask, StepInput, StepReport};
use crate::domain::ports::{Retryable, StepAgent, StepExecutor};

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
    #[error("the agent failed: {report:?}")]
    Agent {
        /// Why.
        report: rootcause::Report,
        /// Whether asking the agent again may succeed.
        retryable: bool,
    },
    /// The agent ended the Step with an outcome the Step does not have.
    #[error("the agent ended step {step} with {outcome}, which it does not end with")]
    UnexpectedOutcome {
        /// The Step.
        step: StepName,
        /// The outcome the agent handed back.
        outcome: Outcome,
    },
}

impl BuiltInStepError {
    fn agent(error: impl std::error::Error + Retryable + Send + Sync + 'static) -> Self {
        BuiltInStepError::Agent {
            retryable: error.retryable(),
            report: rootcause::Report::new(error).into_dynamic(),
        }
    }
}

impl Retryable for BuiltInStepError {
    fn retryable(&self) -> bool {
        match self {
            BuiltInStepError::Agent { retryable, .. } => *retryable,
            BuiltInStepError::UnknownStep(_) | BuiltInStepError::UnexpectedOutcome { .. } => false,
        }
    }
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
            key: input.key,
            owner: input.owner.clone(),
            step: step.clone(),
            prompt: prompt(instructions, &outcomes, input),
            outcomes: outcomes.clone(),
        };
        let answer = self
            .agent
            .answer(task)
            .await
            .map_err(BuiltInStepError::agent)?;

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

    async fn stop(
        &self,
        step: &StepName,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<(), BuiltInStepError> {
        match BuiltInStep::named(step) {
            Some(BuiltInStep::Plan | BuiltInStep::Write) => self
                .agent
                .stop(key, owner)
                .await
                .map_err(BuiltInStepError::agent),
            // A reply is done before anything could stop it.
            Some(BuiltInStep::Reply) | None => Ok(()),
        }
    }
}

/// The most bytes of the Run's earlier Steps a prompt carries.
const HISTORY_BUDGET: usize = 48 * 1024;

/// The agent's prompt: the Step's instructions, the outcomes it may end
/// with, the Run's request, then the Run's earlier Steps ([`history`]).
fn prompt(instructions: &str, outcomes: &[Outcome], input: StepInput<'_>) -> String {
    let outcomes = outcomes
        .iter()
        .map(|outcome| format!("`{outcome}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{instructions}\nEnd the step with one of these outcomes: {outcomes}.\n\n\
         ## Request\n\n{}\n{}",
        input.request,
        history(input.earlier)
    )
}

/// What the Run's earlier Steps produced and how a person answered their
/// Gates, so a later Step reads the plan it carries out and what people said
/// about it: the latest visit of each entry, newest first, as many as fit
/// in [`HISTORY_BUDGET`]. A loop that visits an entry again and again adds
/// nothing but its latest round.
fn history(earlier: &[Step]) -> String {
    let mut latest: Vec<&Step> = Vec::new();
    for step in earlier.iter().rev() {
        if step.end.is_some() && !latest.iter().any(|kept| kept.entry == step.entry) {
            latest.push(step);
        }
    }

    let mut history = String::new();
    let mut left_out = 0;
    for step in latest {
        let section = section(step);
        if history.len() + section.len() > HISTORY_BUDGET {
            left_out += 1;
            continue;
        }
        history.push_str(&section);
    }
    if left_out > 0 {
        history.push_str(&format!(
            "\n({left_out} earlier steps are left out: the prompt has no room for them.)\n"
        ));
    }
    history
}

/// One earlier Step: its outcome and output, and its Gate's answer.
fn section(step: &Step) -> String {
    let Some(end) = &step.end else {
        return String::new();
    };
    let mut section = format!(
        "\n## Step {}, visit {}: {}\n\n{}\n",
        step.entry, step.visit, end.outcome, end.output
    );

    let answer = step
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref().map(|answer| (&gate.question, answer)));
    if let Some((question, answer)) = answer {
        let note = answer.note.as_deref().unwrap_or("(no note)");
        section.push_str(&format!(
            "\nA person was asked \"{question}\" and answered `{}`: {note}\n",
            answer.answer
        ));
    }
    section
}
