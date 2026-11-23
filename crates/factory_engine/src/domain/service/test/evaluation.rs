//! Experiments' arms and Benchmark trials as the Run API starts Runs.

use std::sync::Arc;

use factory_records::domain::evaluation::{
    AgentChoices, BENCHMARK_FLOW, EvaluationRepository, Judgment, TrialKey,
};
use factory_records::domain::models::{AgentName, Role};
use serde_json::json;
use uuid::Uuid;

use super::{FakeExecutor, FakeWorkflows, code_service, julia, julia_s_run, teo};
use crate::domain::agents::AgentDefinition;
use crate::domain::evaluation::insights::calibration::{ScorerLabelError, ScorerLabelService};
use crate::domain::evaluation::insights::{EvaluationService, EvaluationServiceImpl, SuiteError};
use crate::domain::evaluation::{EvaluationDefinition, EvaluationDefinitionError};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;
use crate::domain::service::FactoryServiceImpl;

const REPO_URL: &str = "https://github.com/macro/app";
const BASE: &str = "abcdefabcdefabcdefabcdefabcdefabcdefabcd";

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
            None,
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
            None,
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
            None,
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
            .list_flows(&julia())
            .await
            .expect("flows")
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
            None,
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
            BASE.to_string(),
            None,
        )
        .await
        .expect("trial started");
    let kept = repo.evaluation.lock().expect("evaluation").clone();
    assert_eq!(
        kept.trials[&(Uuid::from_u128(600), "typo".to_string())],
        created.run.id
    );
    assert_eq!(
        kept.base_commits[&(Uuid::from_u128(600), "typo".to_string())],
        BASE
    );
    // A trial asks no one: its agents are offered no question.
    let started = workflows.started.lock().expect("started").clone();
    assert_eq!(started.len(), 1);
    assert!(started[0].flow.unattended);
    assert_eq!(started[0].flow.questions, None);
}

/// A Benchmark of two tasks on [`REPO_URL`], one pinned to a commit, and
/// the agent definition its configurations name Agents of.
fn suites(
    repo: &FakeRepo,
    github: &FakeGithub,
) -> EvaluationServiceImpl<
    FakeRepo,
    FactoryServiceImpl<FakeRepo, FakeWorkflows, FakeExecutor, FakeRepo, FakeRepo, FakeGithub>,
> {
    let definition: EvaluationDefinition = serde_json::from_value(json!({ "benchmarks": [{
        "name": "core",
        "repository": REPO_URL,
        "tasks": [
            { "name": "export", "request": "Add a CSV export.", "baseCommit": PINNED },
            { "name": "typo", "request": "Fix the typo in the README." }
        ]
    }]}))
    .expect("a definition");
    let agents: AgentDefinition = serde_json::from_value(json!({
        "agents": {
            "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
            "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
        },
        "roles": { "writer": ["primary"] }
    }))
    .expect("an agent definition");
    github.state().head = Some(HEAD.to_string());
    EvaluationServiceImpl::new(
        repo.clone(),
        Arc::new(code_service(repo, &FakeWorkflows::default(), github)),
        Arc::new(definition),
        Some(agents),
    )
}

const PINNED: &str = "0123456789012345678901234567890123456789";
const HEAD: &str = "9999999999999999999999999999999999999999";

fn writer(agent: &str) -> AgentChoices {
    [(
        Role::try_from("writer".to_string()).expect("valid"),
        AgentName::try_from(agent.to_string()).expect("valid"),
    )]
    .into_iter()
    .collect()
}

#[tokio::test]
async fn a_suite_starts_a_trial_of_each_task_from_its_pinned_commit_or_the_head() {
    let repo = FakeRepo::default();
    let service = suites(&repo, &FakeGithub::default());

    let started = service
        .start_suite(julia(), "core", writer("primary"), None)
        .await
        .expect("started");

    assert_eq!(started.refused, None);
    assert_eq!(started.runs.len(), 2);
    let kept = repo.evaluation.lock().expect("evaluation").clone();
    assert_eq!(
        kept.base_commits[&(started.id, "export".to_string())],
        PINNED
    );
    assert_eq!(kept.base_commits[&(started.id, "typo".to_string())], HEAD);

    // Its trials still run: a second suite of the Benchmark waits.
    let again = service
        .start_suite(julia(), "core", writer("primary"), None)
        .await;
    assert!(matches!(again, Err(SuiteError::Running(_))), "{again:?}");
}

#[tokio::test]
async fn a_suite_is_refused_for_an_unknown_benchmark_agent_or_repository() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let service = suites(&repo, &github);

    let unknown = service
        .start_suite(julia(), "nope", writer("primary"), None)
        .await;
    assert!(
        matches!(unknown, Err(SuiteError::UnknownBenchmark(_))),
        "{unknown:?}"
    );
    let agent = service
        .start_suite(julia(), "core", writer("nobody"), None)
        .await;
    assert!(
        matches!(agent, Err(SuiteError::Configuration(_))),
        "{agent:?}"
    );
    // A configuration may not give a Role an Agent on Codex's cloud
    // agent: a trial has no other Agent to fall back on.
    let cloud = service
        .start_suite(julia(), "core", writer("backup"), None)
        .await;
    assert!(
        matches!(
            cloud,
            Err(SuiteError::Configuration(
                EvaluationDefinitionError::CloudCodexAgent { .. }
            ))
        ),
        "{cloud:?}"
    );
    github.state().unreachable = true;
    let unreachable = service
        .start_suite(julia(), "core", writer("primary"), None)
        .await;
    assert!(
        matches!(
            unreachable,
            Err(SuiteError::Run(FactoryError::UnreachableRepository))
        ),
        "{unreachable:?}"
    );
    assert!(
        repo.evaluation
            .lock()
            .expect("evaluation")
            .suites
            .is_empty()
    );
}

#[tokio::test]
async fn a_refused_trial_stops_the_suite_and_a_suite_with_none_started_is_not_kept() {
    let repo = FakeRepo::default();
    let service = suites(&repo, &FakeGithub::default());

    repo.fail_once_after("insert_trial", 1);
    let partial = service
        .start_suite(julia(), "core", writer("primary"), None)
        .await
        .expect("the first trial started");
    assert_eq!(partial.runs.keys().collect::<Vec<_>>(), ["export"]);
    let refused = partial.refused.expect("the second trial was refused");
    assert_eq!(refused.task, "typo");
    // The suite plans only the trial it started.
    assert_eq!(
        repo.evaluation.lock().expect("evaluation").suites[&partial.id].planned,
        1
    );

    let other = FakeRepo::default();
    let service = suites(&other, &FakeGithub::default());
    other.fail_once_after("insert_trial", 0);
    let none = service
        .start_suite(julia(), "core", writer("primary"), None)
        .await;
    assert!(matches!(none, Err(SuiteError::Run(_))), "{none:?}");
    assert!(
        other
            .evaluation
            .lock()
            .expect("evaluation")
            .suites
            .is_empty()
    );
}

fn judgment(scorer: &str, score: f64) -> Judgment {
    Judgment {
        scorer: scorer.to_string(),
        score,
        reason: "It does what was asked.".to_string(),
        model: "anthropic/claude-haiku-5-5".to_string(),
    }
}

/// A Run's owner labels the live judgments of it, a new label replacing
/// the last, and reads them back with each Scorer's agreement with their
/// labels. A Scorer that did not judge the Run takes no label, another
/// person reads and labels nothing of the Run, and an agent's account
/// labels nothing even of its own Run.
#[tokio::test]
async fn a_runs_owner_labels_its_judgments_and_reads_the_scorers_agreement() {
    let repo = FakeRepo::default();
    let service = EvaluationServiceImpl::new(
        repo.clone(),
        Arc::new(code_service(
            &repo,
            &FakeWorkflows::default(),
            &FakeGithub::default(),
        )),
        Arc::new(EvaluationDefinition::default()),
        None,
    );
    let run_id = julia_s_run(&repo).await;
    repo.record_run_score(run_id, &judgment("correctness", 0.9))
        .await
        .expect("scored");
    repo.record_run_score(run_id, &judgment("tidiness", 0.2))
        .await
        .expect("scored");

    service
        .label_score(&julia(), run_id, "correctness", false)
        .await
        .expect("labelled");
    let replaced = service
        .label_score(&julia(), run_id, "correctness", true)
        .await
        .expect("labelled");
    assert!(replaced.passed);
    service
        .label_score(&julia(), run_id, "tidiness", true)
        .await
        .expect("labelled");
    let unjudged = service
        .label_score(&julia(), run_id, "slop", true)
        .await
        .expect_err("slop did not judge the run");
    assert!(matches!(unjudged, ScorerLabelError::NotJudged(scorer) if scorer == "slop"));

    let read = service.run_scores(&julia(), run_id).await.expect("read");
    let scores: Vec<_> = read
        .scores
        .iter()
        .map(|score| {
            (
                score.scorer.as_str(),
                score.passed,
                score.label.map(|label| label.passed),
                score.calibration.labelled,
                score.calibration.agreed,
                score.calibration.calibrated,
            )
        })
        .collect();
    assert_eq!(
        scores,
        [
            ("correctness", true, Some(true), 1, 1, false),
            ("tidiness", false, Some(true), 1, 0, false),
        ]
    );

    let by_teo = service
        .label_score(&teo(), run_id, "correctness", false)
        .await
        .expect_err("not teo's run");
    assert!(matches!(by_teo, ScorerLabelError::NotFound));
    let read_by_teo = service
        .run_scores(&teo(), run_id)
        .await
        .expect_err("not teo's run");
    assert!(matches!(read_by_teo, ScorerLabelError::NotFound));

    repo.agent_accounts
        .lock()
        .expect("agent accounts")
        .insert(julia().as_ref().to_string());
    let by_agent = service
        .label_score(&julia(), run_id, "correctness", false)
        .await
        .expect_err("an agent's account labels nothing");
    assert!(matches!(by_agent, ScorerLabelError::NotAPerson));
    let kept = service.run_scores(&julia(), run_id).await.expect("read");
    assert_eq!(kept.scores[0].label.map(|label| label.passed), Some(true));
}
