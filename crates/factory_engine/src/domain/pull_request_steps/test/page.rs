//! The page of a Run's pull request, which the Run keeps for the board.

use factory_records::domain::models::{NewRun, RunId};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;

use super::{JULIA, input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};

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

/// A page the records could not keep does not fail the Step.
#[tokio::test]
async fn a_page_that_cannot_be_kept_does_not_fail_the_step() {
    let github = FakeGithub::default();
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo).await;
    repo.fail_once_after("set_pull_request_url", 0);

    let report = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");

    assert_eq!(outcome(&report), "published");
    assert_eq!(kept_page(&repo, run_id).await, None);
}
