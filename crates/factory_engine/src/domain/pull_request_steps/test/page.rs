//! The page of a Run's pull request, which the Run keeps for the board.

use factory_records::domain::models::{NewRun, RunId};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;

use super::{JULIA, PullRequestStepError, input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::ports::Retryable;

async fn keep_run(repo: &FakeRepo) -> RunId {
    let run_id = RunId::generate();
    repo.insert_run(NewRun {
        id: run_id,
        flow: "code",
        request: "Collapse repeated dashes in slugs",
        user_id: &JULIA,
        task_id: None,
        project_id: None,
        repository: None,
        created_in_person: true,
        control_file_patterns: &[],
        control_files_team_id: None,
        standing_rules: RunRules::DEFAULT,
        automation: None,
        definition: None,
        kept_flow: None,
    })
    .await
    .expect("insert");
    run_id
}

async fn kept_page(repo: &FakeRepo, run_id: RunId) -> Option<String> {
    repo.get_run(run_id)
        .await
        .expect("get")
        .expect("kept")
        .pull_request_url
}

/// Publishing keeps the page of the pull request it opened on the Run.
#[tokio::test]
async fn publishing_keeps_the_pull_request_s_page() {
    let github = FakeGithub::default();
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo).await;

    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");

    assert_eq!(
        kept_page(&repo, run_id).await.as_deref(),
        Some("https://github.com/macro/app/pull/7")
    );
}

/// `ci`, which reads the pull request with its checks, keeps its page too.
#[tokio::test(start_paused = true)]
async fn ci_keeps_the_pull_request_s_page() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo).await;

    steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert!(
        kept_page(&repo, run_id)
            .await
            .is_some_and(|page| page.starts_with("https://github.com/macro/app/pull/"))
    );
}

/// A page the records could not keep fails the Step with an error that is
/// retried, since the merge guard finds the Run by it; the retry keeps it.
#[tokio::test]
async fn a_page_that_cannot_be_kept_fails_the_step_until_it_is_kept() {
    let github = FakeGithub::default();
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo).await;
    repo.fail_once_after("set_pull_request_url", 0);

    let error = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect_err("the page was not kept");
    assert!(matches!(error, PullRequestStepError::Records(_)));
    assert!(error.retryable());
    assert_eq!(kept_page(&repo, run_id).await, None);

    let report = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish again");
    assert_eq!(outcome(&report), "published");
    assert_eq!(
        kept_page(&repo, run_id).await.as_deref(),
        Some("https://github.com/macro/app/pull/7")
    );
}

/// `ci` fails the same way when the page is not kept.
#[tokio::test(start_paused = true)]
async fn ci_fails_when_the_page_cannot_be_kept() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo).await;
    repo.fail_once_after("set_pull_request_url", 0);

    let error = steps(&github, &repo)
        .ci(input(run_id))
        .await
        .expect_err("the page was not kept");
    assert!(matches!(error, PullRequestStepError::Records(_)));
}

/// `ci` reads the pull request on every poll, but keeps its page once: a
/// later read of the same page writes nothing, so a write that would fail
/// then does not start the wait over.
#[tokio::test(start_paused = true)]
async fn ci_keeps_the_same_page_once_across_its_polls() {
    let github = FakeGithub::with_open_pull_request(ci(None));
    github.state().checks = [ci(None), ci(Some("success")), ci(Some("success"))].into();
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo).await;
    repo.fail_once_after("set_pull_request_url", 1);

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(github.state().reads >= 3);
    assert_eq!(
        kept_page(&repo, run_id).await.as_deref(),
        Some("https://github.com/macro/app/pull/7")
    );
}
