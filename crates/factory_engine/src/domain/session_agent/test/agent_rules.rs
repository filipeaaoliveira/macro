//! A Run's Agent Rules choose the Agent its Steps of a Role start with: the
//! rule its task's tags or repository matched as it started, before the
//! Role's own Agents, which still follow it on lack of quota. An Experiment
//! arm's choice comes before the rule's.

use agent_session::domain::routines::RoutineActionStatus;
use bot_id::BotId;
use factory_records::domain::evaluation::{EvaluationRepository, RunArm};
use factory_records::domain::models::{PassedOver, Role};
use uuid::Uuid;

use super::{FakeSessions, PRIMARY, SPARE, agent, name, no_quota, plan_task, recorded, the_plan};
use crate::domain::agents::{AgentDefinition, RuleSubject};
use crate::domain::fakes::FakeRepo;
use crate::domain::flow::StepName;
use crate::domain::models::AgentTask;
use crate::domain::ports::StepAgent;

/// The bot whose sessions `docs-writer` runs in.
const DOCS_WRITER: BotId = BotId::new_from_uuid(Uuid::from_u128(0xd0c5));

fn writer() -> Role {
    Role::try_from("writer".to_string()).expect("valid")
}

/// The Definition a Run kept: `primary`, `backup`, then `docs-writer`
/// write, `spare` is only a Standing Role's, and a task tagged `docs` is
/// written by `docs-writer` first.
fn kept() -> serde_json::Value {
    serde_json::json!({
        "agents": {
            "primary": { "bot": Uuid::from_u128(0xc1a0) },
            "backup": { "bot": Uuid::from_u128(0xc0de) },
            "spare": { "bot": Uuid::from_u128(0x5a5e) },
            "docs-writer": { "bot": Uuid::from_u128(0xd0c5), "model": "small-model" }
        },
        "roles": { "planner": ["primary"], "writer": ["primary", "backup", "docs-writer"] },
        "standing": { "intake": { "acts": ["start"], "agents": ["spare"] } },
        "rules": [
            { "name": "docs", "role": "writer", "tag": "docs", "agent": "docs-writer" }
        ]
    })
}

/// A `write` task of a new Run of Julia's under the kept Definition, its
/// task carrying `tags`, its Step started, with the rules the Run's start
/// recorded: those `tags` matched, past the Roles its `arms` chose for.
async fn write_task(repo: &FakeRepo, tags: &[&str], arms: &[RunArm]) -> AgentTask {
    let mut task = plan_task(repo).await;
    task.step = StepName::try_from("write".to_string()).expect("valid");
    task.role = writer();
    task.prompt = "Write the change".to_string();
    repo.definition_agents
        .lock()
        .expect("definition agents")
        .insert(task.key.run_id, kept());
    repo.record_arms(task.key.run_id, arms)
        .await
        .expect("arms kept");
    let definition: AgentDefinition = serde_json::from_value(kept()).expect("a definition");
    let tags: Vec<String> = tags.iter().map(ToString::to_string).collect();
    let rules = definition.chosen_by_rules(
        RuleSubject {
            tags: &tags,
            repository: None,
        },
        |role| arms.iter().any(|arm| arm.agents.contains_key(role)),
    );
    repo.record_agent_rules(task.key.run_id, &rules)
        .await
        .expect("rules kept");
    task
}

/// The bots of the sessions the Step prepared, in order.
fn prepared_bots(sessions: &FakeSessions) -> Vec<BotId> {
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    prepared.iter().map(|call| call.selection.bot_id).collect()
}

/// A write Step of a Run whose task is tagged `docs` starts with the
/// `docs` rule's Agent, on its model; when that Agent cannot start for lack
/// of quota, the Role's own first Agent takes the Step, as before, and the
/// Step records the rule's Agent passed over.
#[tokio::test]
async fn a_tagged_task_s_write_step_starts_with_the_rule_s_agent_and_falls_back_on_quota() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = write_task(&repo, &["Docs"], &[]).await;
    let key = task.key;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [DOCS_WRITER]);
    let model = sessions.prepared_calls.lock().expect("prepared")[0]
        .selection
        .model
        .clone();
    assert_eq!(model.as_deref(), Some("small-model"));
    assert_eq!(recorded(&repo, key).1, Some(name("docs-writer")));

    // Out of quota, the rule's Agent hands the Step to the Role's first.
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let task = write_task(&repo, &["docs"], &[]).await;
    let key = task.key;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [DOCS_WRITER, PRIMARY]);
    assert_eq!(recorded(&repo, key).1, Some(name("primary")));
    assert_eq!(
        recorded(&repo, key).2,
        [PassedOver {
            agent: name("docs-writer"),
            reason: "ai_allowance_exhausted".to_string(),
        }]
    );
}

/// A task no rule matches keeps the Role's own order.
#[tokio::test]
async fn a_task_no_rule_matches_keeps_the_role_s_first_agent() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = write_task(&repo, &["urgent"], &[]).await;
    let key = task.key;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [PRIMARY]);
    assert_eq!(recorded(&repo, key).1, Some(name("primary")));
}

/// An Experiment arm that chooses the Role's Agent overrides the rule: the
/// Step starts with the arm's Agent, then the Role's own.
#[tokio::test]
async fn an_arm_s_choice_overrides_the_rule() {
    let arm = RunArm {
        experiment: "writer-agent".to_string(),
        arm: "spare".to_string(),
        agents: [(writer(), name("spare"))].into_iter().collect(),
    };
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let task = write_task(&repo, &["docs"], std::slice::from_ref(&arm)).await;
    let key = task.key;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [SPARE, PRIMARY]);
    assert_eq!(recorded(&repo, key).1, Some(name("primary")));

    // A Run that kept both, as one started before the start left an arm's
    // Role out, still starts with the arm's.
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = write_task(&repo, &["docs"], &[]).await;
    repo.record_arms(task.key.run_id, &[arm])
        .await
        .expect("arm kept");

    agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(prepared_bots(&sessions), [SPARE]);
}
