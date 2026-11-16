use std::sync::{Arc, LazyLock, Mutex};

use chrono::Utc;
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, Outcome, RunId, Step, StepEnd, StepKey,
};
use macro_user_id::user_id::MacroUserIdStr;

use super::{BuiltInStepError, BuiltInSteps};
use crate::domain::flow::{Flow, StepName};
use crate::domain::models::{AgentAnswer, AgentTask, StepInput};
use crate::domain::ports::{Retryable, StepAgent, StepExecutor};

#[derive(Debug, thiserror::Error)]
#[error("fake agent failure")]
struct FakeError;

impl Retryable for FakeError {
    fn retryable(&self) -> bool {
        true
    }
}

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

    async fn stop(&self, _key: StepKey, _owner: &MacroUserIdStr<'static>) -> Result<(), FakeError> {
        Ok(())
    }

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

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

fn input<'a>(earlier: &'a [Step]) -> StepInput<'a> {
    StepInput {
        key: StepKey {
            run_id: RunId::generate(),
            seq: 2,
        },
        owner: &JULIA,
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
        seq: 1,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "Add a toggle to the settings page".to_string(),
            ended_at: Utc::now(),
        }),
        gate: None,
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

#[tokio::test]
async fn a_plan_reads_what_a_person_answered_the_last_one() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone());
    let revise = Outcome::try_from("revise".to_string()).expect("valid");
    let earlier = [Step {
        seq: 1,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "Rebuild the index".to_string(),
            ended_at: Utc::now(),
        }),
        gate: Some(Gate {
            question: "Approve the plan?".to_string(),
            answers: vec![Outcome::done(), revise.clone()],
            opened_at: Utc::now(),
            answer: Some(GateAnswer {
                answer: revise,
                note: Some("Keep the old index".to_string()),
                user_id: None,
                answered_at: Utc::now(),
            }),
        }),
    }];

    steps
        .run(&step("plan"), input(&earlier))
        .await
        .expect("plan");

    let tasks = agent.tasks.lock().expect("tasks");
    assert!(tasks[0].prompt.contains("revise"));
    assert!(tasks[0].prompt.contains("Keep the old index"));
}

/// An ended Step of entry `entry` at place `seq`, which produced `output`.
fn ended(seq: u16, entry: &str, visit: u16, output: &str) -> Step {
    Step {
        seq,
        entry: EntryId::try_from(entry.to_string()).expect("valid"),
        visit,
        started_at: Utc::now(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: output.to_string(),
            ended_at: Utc::now(),
        }),
        gate: None,
    }
}

/// A loop's earlier rounds are history the agent doesn't need, and a Run
/// that loops would otherwise grow its prompt without bound.
#[tokio::test]
async fn a_prompt_carries_the_latest_visit_of_each_entry_newest_first() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone());
    let earlier = [
        ended(1, "plan", 1, "First plan"),
        ended(2, "review", 1, "First review"),
        ended(3, "plan", 2, "Second plan"),
        ended(4, "review", 2, "Second review"),
    ];

    steps
        .run(&step("write"), input(&earlier))
        .await
        .expect("write");

    let tasks = agent.tasks.lock().expect("tasks");
    let prompt = &tasks[0].prompt;
    assert!(!prompt.contains("First plan"));
    assert!(!prompt.contains("First review"));
    let review = prompt.find("Second review").expect("the latest review");
    let plan = prompt.find("Second plan").expect("the latest plan");
    assert!(review < plan, "newest first");
}

#[tokio::test]
async fn a_prompt_leaves_out_the_oldest_steps_past_its_budget() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone());
    let earlier = [
        ended(1, "plan", 1, &"p".repeat(40 * 1024)),
        ended(2, "review", 1, &"r".repeat(40 * 1024)),
    ];

    steps
        .run(&step("write"), input(&earlier))
        .await
        .expect("write");

    let tasks = agent.tasks.lock().expect("tasks");
    let prompt = &tasks[0].prompt;
    assert!(prompt.contains(&"r".repeat(40 * 1024)));
    assert!(!prompt.contains(&"p".repeat(40 * 1024)));
    assert!(prompt.contains("1 earlier steps are left out"));
}
