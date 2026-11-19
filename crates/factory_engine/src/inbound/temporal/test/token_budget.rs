//! A Run's token budget, checked by the workflow before each agent Step:
//! a Run that used it ends `blocked` before its next agent Step, with why
//! kept and its owner told; a usage log that cannot be read lets a few
//! Steps run, then ends the Run; a Run without a budget walks as before,
//! with no check and no marker in its history.

use std::sync::Arc;

use factory_records::domain::models::{BudgetStop, NewRun, RunId, RunInsert, RunOutcome};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules};
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use temporalio_client::errors::WorkflowGetResultError;
use temporalio_client::{
    WorkflowFetchHistoryOptions, WorkflowGetResultOptions, WorkflowStartOptions,
};
use temporalio_sdk::Runtime;
use tokio::sync::Notify;
use uuid::Uuid;

use super::{Agent, PATIENCE, Work, client, gates, record_history, scoring, steps_of, watches};
use crate::domain::fakes::{FakeGithub, FakeNotifier, FakeRepo, FakeRunTokens, FakeTaskNotes};
use crate::domain::flow::Flow;
use crate::domain::models::RunStart;
use crate::domain::review_rounds::ReviewRoundsImpl;
use crate::domain::service::StepServiceImpl;
use crate::domain::token_budget::MAX_UNREAD_CHECKS;
use crate::inbound::temporal::{WorkerServices, worker_on};
use crate::temporal::FlowWorkflowDefinition;

const BUDGET: u64 = 100_000;

/// The `factory-token-budget` marker as a history keeps it: its patch data,
/// `{"id":"factory-token-budget","deprecated":false}`, in base64.
const MARKER_IN_HISTORY: &str = "eyJpZCI6ImZhY3RvcnktdG9rZW4tYnVkZ2V0IiwiZGVwcmVjYXRlZCI6ZmFsc2V9";

/// How many agent entries the Run's Flow walks, one after the other.
const ENTRIES: usize = 4;

/// What a walk left: how the workflow ended, the records, the Run, the
/// notices its owner was sent, and its history.
struct Walked {
    ended: Result<RunOutcome, WorkflowGetResultError>,
    repo: FakeRepo,
    run_id: RunId,
    notifier: FakeNotifier,
    history: String,
}

/// Walk a Run of Julia's under a token budget of `budget`, whose Flow has
/// [`ENTRIES`] agent entries, each ending `done` into the next, with an
/// agent that ends each, and the usage log `usage`.
async fn walk(budget: Option<u64>, usage: &FakeRunTokens) -> Walked {
    let client = client().await;
    let repo = FakeRepo::default();
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid");
    let rules = RunRules {
        definition: OrchestratorDefinition {
            token_budget: budget,
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    };
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Add a dark mode toggle",
            user_id: &julia,
            task_id: Some(task),
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run on a task no other run works on is kept");
    };
    let entries: Vec<_> = (1..=ENTRIES)
        .map(|place| {
            let then = if place == ENTRIES {
                json!({ "kind": "end", "outcome": "success" })
            } else {
                json!({ "kind": "entry", "entry": format!("plan-{}", place + 1) })
            };
            json!({
                "id": format!("plan-{place}"),
                "step": "plan",
                "role": "planner",
                "on": { "done": [{ "to": then }] }
            })
        })
        .collect();
    let flow = Flow::parse(json!({ "entries": entries }), |_| {
        Some(crate::domain::flow::StepUse::Work)
    })
    .expect("valid flow");
    let start = RunStart {
        run_id: run.id,
        flow,
        token_budget: budget.is_some(),
    };
    let run_id = run.id;

    let agent = Agent::doing(Work::Ends);
    let notifier = FakeNotifier::default();
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        WorkerServices {
            steps: StepServiceImpl::new(repo.clone(), steps_of(&agent, &repo))
                .with_owner_gates(notifier.clone(), FakeTaskNotes::default())
                .with_usage(usage.clone()),
            reviews: ReviewRoundsImpl::new(repo.clone(), FakeGithub::default(), repo.clone()),
            gates: gates(&repo),
            watches: watches(&repo),
            scoring: scoring(&repo),
        },
    )
    .expect("worker");

    let done = Arc::new(Notify::new());
    let walked = async {
        let handle = client
            .start_workflow(
                FlowWorkflowDefinition,
                start,
                WorkflowStartOptions::new(task_queue.clone(), run_id.to_string()).build(),
            )
            .await
            .expect("start");
        let ended = tokio::time::timeout(
            PATIENCE,
            handle.get_result(WorkflowGetResultOptions::default()),
        )
        .await
        .expect("the workflow ends");
        done.notify_one();
        ended
    };
    let (worked, ended) = tokio::join!(worker.run_until(done.notified()), walked);
    worked.expect("the worker ran");
    let history = client
        .get_workflow_handle::<FlowWorkflowDefinition>(run_id.to_string())
        .fetch_history(WorkflowFetchHistoryOptions::default())
        .to_json()
        .await
        .expect("the history reads");
    Walked {
        ended,
        repo,
        run_id,
        notifier,
        history: String::from_utf8_lossy(history.as_ref()).into_owned(),
    }
}

/// How many Steps the Run took.
async fn steps_taken(walked: &Walked) -> usize {
    walked
        .repo
        .list_steps(walked.run_id)
        .await
        .expect("steps")
        .len()
}

/// A Run that used its budget by the time its second agent Step would
/// start ends `blocked` there: the first Step ran, the second never
/// started, the Run keeps how much it used, and its owner is told. Its
/// history, which passed the `factory-token-budget` marker, is kept for
/// the replay tests.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_run_at_its_budget_ends_blocked_before_its_next_agent_step() {
    let usage = FakeRunTokens::reading([Some(0), Some(BUDGET + 1)]);

    let walked = walk(Some(BUDGET), &usage).await;

    assert_eq!(
        walked.ended.as_ref().expect("the workflow ends"),
        &RunOutcome::Blocked
    );
    assert_eq!(steps_taken(&walked).await, 1);
    let run = walked
        .repo
        .get_run(walked.run_id)
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Blocked));
    assert_eq!(
        run.budget_stop,
        Some(BudgetStop::Spent { used: BUDGET + 1 })
    );
    assert_eq!(usage.reads(), 2);
    let sent = walked.notifier.sent.lock().expect("sent").clone();
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].metadata.question.contains("token budget"));
    assert!(walked.history.contains(MARKER_IN_HISTORY));
    assert!(walked.history.contains("factory_check_budget"));
    record_history(walked.run_id, "plan_stopped_at_token_budget").await;
}

/// A usage log that cannot be read lets the Run's agent Steps go on for
/// `MAX_UNREAD_CHECKS - 1` checks in a row; the next check ends it
/// `blocked`, as unread.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_run_whose_usage_cannot_be_read_ends_after_a_few_steps() {
    let usage = FakeRunTokens::reading([None]);

    let walked = walk(Some(BUDGET), &usage).await;

    assert_eq!(
        walked.ended.as_ref().expect("the workflow ends"),
        &RunOutcome::Blocked
    );
    assert_eq!(
        steps_taken(&walked).await,
        usize::from(MAX_UNREAD_CHECKS - 1)
    );
    let run = walked
        .repo
        .get_run(walked.run_id)
        .await
        .expect("read")
        .expect("kept");
    assert_eq!(run.budget_stop, Some(BudgetStop::Unread));
}

/// A Run with no budget walks every Step as before: the usage log is not
/// read, and its history has neither the check nor its marker.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_run_without_a_budget_walks_as_before() {
    let usage = FakeRunTokens::reading([Some(u64::MAX)]);

    let walked = walk(None, &usage).await;

    assert_eq!(
        walked.ended.as_ref().expect("the workflow ends"),
        &RunOutcome::Success
    );
    assert_eq!(steps_taken(&walked).await, ENTRIES);
    assert_eq!(usage.reads(), 0);
    assert!(!walked.history.contains(MARKER_IN_HISTORY));
    assert!(!walked.history.contains("factory_check_budget"));
    assert!(walked.notifier.sent.lock().expect("sent").is_empty());
}
