//! Experiments' arms and Benchmark trials as the Run API starts Runs.

use std::sync::Arc;

use factory_records::domain::evaluation::{BENCHMARK_FLOW, TrialKey};
use serde_json::json;
use uuid::Uuid;

use super::{FakeWorkflows, code_service, julia};
use crate::domain::evaluation::EvaluationDefinition;
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

const REPO_URL: &str = "https://github.com/macro/app";

fn experiment() -> Arc<EvaluationDefinition> {
    Arc::new(
        serde_json::from_value(json!({ "experiments": [{
            "name": "writer-agent",
            "flow": "code",
            "arms": [
                { "name": "control" },
                { "name": "backup", "agents": { "writer": "backup" } }
            ]
        }]}))
        .expect("a definition"),
    )
}

#[tokio::test]
async fn a_run_on_a_task_keeps_its_arm_before_its_workflow_starts() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service =
        code_service(&repo, &workflows, &FakeGithub::default()).with_evaluation(experiment());
    let task_id = Uuid::from_u128(42);
    repo.tasks.lock().expect("tasks").insert(task_id);

    let on_task = service
        .create_run(
            julia(),
            "code".to_string(),
            "Add the export".to_string(),
            Some(task_id),
            Some(REPO_URL.to_string()),
            true,
        )
        .await
        .expect("create");
    let on_no_task = service
        .create_run(
            julia(),
            "code".to_string(),
            "Add the import".to_string(),
            None,
            Some(REPO_URL.to_string()),
            true,
        )
        .await
        .expect("create");

    let kept = repo.evaluation.lock().expect("evaluation").clone();
    assert_eq!(
        kept.arms[&on_task.run.id],
        experiment().assign_arms("code", Some(task_id))
    );
    assert!(!kept.arms.contains_key(&on_no_task.run.id));
    assert_eq!(workflows.started.lock().expect("started").len(), 2);
}

#[tokio::test]
async fn a_run_whose_arm_is_not_kept_does_not_start() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service =
        code_service(&repo, &workflows, &FakeGithub::default()).with_evaluation(experiment());
    let task_id = Uuid::from_u128(42);
    repo.tasks.lock().expect("tasks").insert(task_id);
    repo.fail_once_after("record_arms", 0);

    let refused = service
        .create_run(
            julia(),
            "code".to_string(),
            "Add the export".to_string(),
            Some(task_id),
            Some(REPO_URL.to_string()),
            true,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::Internal(_))));
    assert!(workflows.started.lock().expect("started").is_empty());
    assert!(repo.runs.lock().expect("runs").is_empty());
}

#[tokio::test]
async fn the_benchmark_flow_runs_only_as_a_suite_s_trial() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = code_service(&repo, &workflows, &FakeGithub::default());

    assert!(
        !service
            .list_flows()
            .iter()
            .any(|flow| flow.name == BENCHMARK_FLOW)
    );
    let refused = service
        .create_run(
            julia(),
            BENCHMARK_FLOW.to_string(),
            "Fix the typo".to_string(),
            None,
            Some(REPO_URL.to_string()),
            true,
        )
        .await;
    assert!(matches!(refused, Err(FactoryError::UnknownFlow)));

    let trial = TrialKey {
        suite_id: Uuid::from_u128(600),
        task: "typo".to_string(),
    };
    let created = service
        .create_trial_run(
            julia(),
            BENCHMARK_FLOW.to_string(),
            "Fix the typo".to_string(),
            REPO_URL.to_string(),
            trial,
        )
        .await
        .expect("trial started");
    let kept = repo.evaluation.lock().expect("evaluation").clone();
    assert_eq!(
        kept.trials[&(Uuid::from_u128(600), "typo".to_string())],
        created.run.id
    );
    assert_eq!(workflows.started.lock().expect("started").len(), 1);
}
