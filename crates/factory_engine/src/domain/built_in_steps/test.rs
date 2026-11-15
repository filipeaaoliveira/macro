use std::sync::{Arc, Mutex};

use chrono::Utc;
use factory_records::domain::models::{EntryId, Outcome, RunId, Step, StepEnd};

use super::{BuiltInStepError, BuiltInSteps};
use crate::domain::flow::{Flow, StepName};
use crate::domain::models::{AgentAnswer, AgentTask, StepInput};
use crate::domain::ports::{StepAgent, StepExecutor};

#[derive(Debug, thiserror::Error)]
#[error("fake agent failure")]
struct FakeError;

/// Keeps every task it is given and ends each with `outcome`.
#[derive(Clone)]
struct FakeAgent {
    outcome: &'static str,
    tasks: Arc<Mutex<Vec<AgentTask>>>,
}

impl FakeAgent {
    fn ending_with(outcome: &'static str) -> Self {
        Self {
            outcome,
            tasks: Arc::default(),
        }
    }
}

impl StepAgent for FakeAgent {
    type Err = FakeError;

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, FakeError> {
        self.tasks.lock().expect("tasks").push(task);
        Ok(AgentAnswer {
            outcome: Outcome::try_from(self.outcome.to_string()).expect("valid"),
            text: "The agent's work".to_string(),
        })
    }
}

fn step(name: &str) -> StepName {
    StepName::try_from(name.to_string()).expect("valid")
}

fn input<'a>(earlier: &'a [Step]) -> StepInput<'a> {
    StepInput {
        run_id: RunId::generate(),
        request: "Add a dark mode toggle",
        earlier,
    }
}

#[test]
fn reply_plan_and_write_are_served() {
    let steps = BuiltInSteps::new(FakeAgent::ending_with("done"));

    assert!(steps.serves(&step("reply")));
    assert!(steps.serves(&step("plan")));
    assert!(steps.serves(&step("write")));
    assert!(!steps.serves(&step("review")));
}

#[tokio::test]
async fn a_reply_is_done_and_answers_the_request() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone());

    let report = steps.run(&step("reply"), input(&[])).await.expect("reply");

    assert_eq!(report.outcome, Outcome::done());
    assert!(report.output.contains("Add a dark mode toggle"));
    assert!(agent.tasks.lock().expect("tasks").is_empty());
}

#[tokio::test]
async fn a_plan_hands_the_request_to_the_agent_and_keeps_its_answer() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone());

    let report = steps.run(&step("plan"), input(&[])).await.expect("plan");

    assert_eq!(report.outcome, Outcome::done());
    assert_eq!(report.output, "The agent's work");
    let tasks = agent.tasks.lock().expect("tasks");
    assert_eq!(tasks[0].outcomes, [Outcome::done()]);
    assert!(tasks[0].prompt.contains("Add a dark mode toggle"));
}

#[tokio::test]
async fn a_write_is_given_the_plan_an_earlier_step_produced() {
    let agent = FakeAgent::ending_with("unchanged");
    let steps = BuiltInSteps::new(agent.clone());
    let earlier = [Step {
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "Add a toggle to the settings page".to_string(),
            ended_at: Utc::now(),
        }),
    }];

    let report = steps
        .run(&step("write"), input(&earlier))
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::unchanged());
    let tasks = agent.tasks.lock().expect("tasks");
    assert!(
        tasks[0]
            .prompt
            .contains("Add a toggle to the settings page")
    );
}

#[tokio::test]
async fn an_outcome_the_step_does_not_have_fails_it() {
    let steps = BuiltInSteps::new(FakeAgent::ending_with("merged"));

    let refused = steps.run(&step("plan"), input(&[])).await;

    assert!(matches!(
        refused,
        Err(BuiltInStepError::UnexpectedOutcome { .. })
    ));
}

#[test]
fn the_plan_flow_runs_on_the_built_in_steps() {
    let steps = BuiltInSteps::new(FakeAgent::ending_with("done"));
    let definition = serde_json::from_str(include_str!("../../../flows/plan.json")).expect("JSON");

    let parsed = Flow::parse(definition, |name| steps.serves(name));

    assert!(parsed.is_ok(), "{parsed:?}");
}
