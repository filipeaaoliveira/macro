//! A retry of a `code` Run that lost its writer's session: its workflow
//! starts at `write`, past the plan it kept, on the retried Run's branch,
//! and goes on to merge.

use std::sync::Arc;

use factory_records::domain::models::{
    NewRetry, NewRun, QueueHold, Repository, RetryInsert, RunId, RunInsert, RunOutcome,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;
use temporalio_client::errors::WorkflowGetResultError;
use temporalio_client::{Client, WorkflowGetResultOptions, WorkflowStartOptions};
use temporalio_sdk::Runtime;
use tokio::sync::Notify;
use uuid::Uuid;

use super::{CodeAgent, CodeRun, PATIENCE, client, gates, julia, scoring, watches};
use crate::domain::built_in_flows::BuiltInFlow;
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::flow::{Flow, StepUse};
use crate::domain::models::{RunStart, StartAt, run_branch};
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::review_rounds::{ChangedFile, ReviewRoundsImpl};
use crate::domain::service::StepServiceImpl;
use crate::domain::service::retry::{RetryStart, retry_start};
use crate::inbound::temporal::{WorkerServices, worker_on};
use crate::temporal::FlowWorkflowDefinition;

/// Start `start` on `task_queue`, run `drive` on it, and wait for its end.
async fn walked<F>(
    client: &Client,
    task_queue: &str,
    repo: &FakeRepo,
    start: RunStart,
    drive: impl FnOnce(CodeRun) -> F,
) -> Result<RunOutcome, WorkflowGetResultError>
where
    F: Future<Output = ()>,
{
    let run_id = start.run_id;
    let handle = client
        .start_workflow(
            FlowWorkflowDefinition,
            start,
            WorkflowStartOptions::new(task_queue.to_string(), run_id.to_string()).build(),
        )
        .await
        .expect("start");
    drive(CodeRun {
        client: client.clone(),
        run_id,
        repo: repo.clone(),
    })
    .await;
    tokio::time::timeout(
        PATIENCE,
        handle.get_result(WorkflowGetResultOptions::default()),
    )
    .await
    .expect("the workflow ends")
}

/// A Run stopped by a lost session is retried and merges without planning
/// again: the retry keeps the approved plan, writes on the first Run's
/// branch, and merges the one pull request.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_run_stopped_by_a_lost_session_is_retried_and_merges_without_planning_again() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    agent.lost.lock().expect("lost").push("write");
    let repo = FakeRepo::default();
    let client = client().await;
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Collapse repeated dashes in slugs",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: Some(&repository),
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let flow: Flow = BuiltInFlow::named("code")
        .expect("built in")
        .read(|_| Some(StepUse::Work))
        .expect("valid flow");
    github.state().branch_heads = [Some(FakeGithub::HEAD.to_string())].into();
    github.state().diffs.insert(
        ("main".to_string(), FakeGithub::HEAD.to_string()),
        vec![ChangedFile::from_patch(
            "src/slug.rs".to_string(),
            None,
            Some("@@ -1,3 +1,6 @@"),
        )],
    );
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(
            github.clone(),
            repo.clone(),
            crate::domain::fakes::FakeWatches::default(),
        ),
    );
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        WorkerServices {
            steps: StepServiceImpl::new(repo.clone(), steps),
            reviews: ReviewRoundsImpl::new(repo.clone(), github.clone(), repo.clone()),
            gates: gates(&repo),
            watches: watches(&repo),
            scoring: scoring(&repo),
        },
    )
    .expect("worker");

    let done = Arc::new(Notify::new());
    let both = async {
        // The first Run plans, has its plan approved, and loses its
        // writer.
        let first = walked(
            &client,
            &task_queue,
            &repo,
            RunStart {
                run_id: run.id,
                flow: flow.clone(),
                token_budget: false,
                start_at: None,
            },
            |run| async move { run.answer_next_gate("approve").await },
        )
        .await;

        // Its retry starts where it stopped, as the Run API starts one.
        let taken = repo.list_steps(run.id).await.expect("read");
        let RetryStart::At(entry, first_seq) = retry_start(&flow, &taken, 1) else {
            panic!("a start");
        };
        let RetryInsert::Inserted(retry) = repo
            .retry_run(
                NewRetry {
                    id: RunId::generate(),
                    retried: run.id,
                    first_seq,
                    created_in_person: false,
                },
                QueueHold::UNLIMITED,
            )
            .await
            .expect("retry")
        else {
            panic!("an ended run is retried");
        };
        let second = walked(
            &client,
            &task_queue,
            &repo,
            RunStart {
                run_id: retry.id,
                flow: flow.clone(),
                token_budget: false,
                start_at: Some(StartAt {
                    entry,
                    first_seq,
                    branch_of: run.id,
                }),
            },
            |run| async move {
                let merge_question = run.next_gate().await;
                run.answer(merge_question, "approve").await;
            },
        )
        .await;
        done.notify_one();
        (first, retry.id, second)
    };
    let (worked, (first, retry_id, second)) = tokio::join!(worker.run_until(done.notified()), both);
    worked.expect("the worker ran");

    assert!(matches!(first, Ok(RunOutcome::Infrastructure)), "{first:?}");
    assert!(matches!(second, Ok(RunOutcome::Success)), "{second:?}");
    // The plan ran once: the retry wrote again and went on.
    assert_eq!(
        agent.steps(),
        [
            "plan", "write", "write", "review", "review", "review", "accept"
        ]
    );
    // The retry kept the plan at its place, and took its own Steps after.
    let retried = repo.list_steps(retry_id).await.expect("read");
    assert_eq!(retried[0].entry.as_str(), "plan");
    assert!(
        retried[0]
            .gate
            .as_ref()
            .is_some_and(|gate| gate.answer.is_some())
    );
    assert_eq!(retried[1].seq, 2);
    assert_eq!(retried[1].entry.as_str(), "write");
    // It merged the one pull request, from the first Run's branch.
    let github = github.state();
    assert_eq!(github.opened, 1);
    assert_eq!(github.merges, [FakeGithub::HEAD]);
    assert_eq!(github.deleted_branches, [run_branch(run.id)]);
}
