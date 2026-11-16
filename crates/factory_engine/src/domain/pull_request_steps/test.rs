use std::sync::LazyLock;

use factory_records::domain::models::{Repository, RunId, StepKey};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{CI_WAIT, PullRequestStepError, PullRequestSteps};
use crate::domain::fakes::{FakeGithub, FakeRepo, check};
use crate::domain::models::{PullRequestState, StepInput, StepReport};

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

static REPOSITORY: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/app".to_string(),
    base_branch: "main".to_string(),
});

fn input(run_id: RunId) -> StepInput<'static> {
    StepInput {
        key: StepKey { run_id, seq: 5 },
        owner: &JULIA,
        request: "Collapse repeated dashes in slugs\n\nAnd add a test.",
        task_id: Some(Uuid::parse_str("00000000-0000-7000-8000-000000000101").expect("valid")),
        repository: Some(&REPOSITORY),
        earlier: &[],
    }
}

fn steps(github: &FakeGithub, repo: &FakeRepo) -> PullRequestSteps<FakeGithub, FakeRepo> {
    PullRequestSteps::new(github.clone(), repo.clone())
}

fn outcome(report: &StepReport) -> &str {
    report.outcome.as_str()
}

/// A retried publish, or one a fix sent the Run back to, finds the pull
/// request the first one opened, and opens no other.
#[tokio::test]
async fn publishing_again_finds_the_pull_request_it_opened() {
    let github = FakeGithub::default();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let first = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    let again = steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish again");

    assert_eq!(outcome(&first), "published");
    assert_eq!(outcome(&again), "published");
    assert!(first.output.contains("https://github.com/macro/app/pull/7"));
    assert_eq!(github.state().opened, 1);
}

/// GitHub refusing a pull request another attempt opened first still
/// publishes it; refusing one with nothing to propose ends `refused` with
/// GitHub's reason.
#[tokio::test]
async fn publishing_reads_github_s_refusal() {
    let opened_elsewhere = FakeGithub::with_open_pull_request(Vec::new());
    opened_elsewhere.state().open_refusal = Some("A pull request already exists.".to_string());
    let empty = FakeGithub::default();
    empty.state().open_refusal = Some("No commits between main and the branch.".to_string());
    let repo = FakeRepo::default();

    let published = steps(&opened_elsewhere, &repo)
        .publish(input(RunId::generate()))
        .await
        .expect("publish");
    let refused = steps(&empty, &repo)
        .publish(input(RunId::generate()))
        .await
        .expect("publish");

    assert_eq!(outcome(&published), "published");
    assert_eq!(outcome(&refused), "refused");
    assert_eq!(refused.output, "No commits between main and the branch.");
}

/// A pull request a person closed is not opened again.
#[tokio::test]
async fn a_closed_pull_request_is_not_published_again() {
    let github = FakeGithub::with_open_pull_request(Vec::new());
    if let Some(pull_request) = github.state().pull_request.as_mut() {
        pull_request.state = PullRequestState::Closed;
    }

    let report = steps(&github, &FakeRepo::default())
        .publish(input(RunId::generate()))
        .await
        .expect("publish");

    assert_eq!(outcome(&report), "closed");
    assert_eq!(github.state().opened, 0);
}

/// A Run with no repository has nothing to publish, for good.
#[tokio::test]
async fn a_run_with_no_repository_publishes_nothing() {
    let mut no_repository = input(RunId::generate());
    no_repository.repository = None;

    let failed = steps(&FakeGithub::default(), &FakeRepo::default())
        .publish(no_repository)
        .await;

    assert!(matches!(failed, Err(PullRequestStepError::NoRepository)));
}

/// CI ends `red` as soon as a check fails, naming the failing checks only.
#[tokio::test(start_paused = true)]
async fn ci_is_red_with_the_checks_that_failed() {
    let github = FakeGithub::with_open_pull_request(vec![
        check("lint", Some("success")),
        check("test", Some("failure")),
        check("build", None),
    ]);

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(
        report
            .output
            .contains("- test (failure): https://github.com/macro/app/runs/test")
    );
    assert!(!report.output.contains("lint"));
    assert!(!report.output.contains("build"));
}

/// CI waits while checks run, or before any is reported, and ends `green`
/// once every one passed or was skipped.
#[tokio::test(start_paused = true)]
async fn ci_waits_for_the_checks_then_is_green() {
    let github = FakeGithub::with_open_pull_request(Vec::new());
    github.state().checks = [
        Vec::new(),
        vec![check("test", None)],
        vec![
            check("test", Some("success")),
            check("docs", Some("skipped")),
        ],
    ]
    .into();

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(report.output.contains(FakeGithub::HEAD));
    assert_eq!(github.state().checks.len(), 1);
}

/// Checks that never finish end CI `timeout` once it waited [`CI_WAIT`].
#[tokio::test(start_paused = true)]
async fn ci_that_never_finishes_times_out() {
    let github = FakeGithub::with_open_pull_request(vec![check("test", None)]);
    let started = tokio::time::Instant::now();

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "timeout");
    assert!(started.elapsed() <= CI_WAIT);
}

/// The commit whose checks passed is the one a person is asked to merge,
/// and the one kept for the merge.
#[tokio::test]
async fn ready_to_merge_keeps_the_commit_whose_checks_passed() {
    let github = FakeGithub::with_open_pull_request(vec![check("test", Some("success"))]);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let report = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "done");
    assert!(report.output.contains(FakeGithub::HEAD));
    assert_eq!(
        repo.merge_head(run_id).await.expect("read").as_deref(),
        Some(FakeGithub::HEAD)
    );
}

/// A head pushed to after CI passed, whose checks have not, goes back to CI
/// rather than to a person.
#[tokio::test]
async fn ready_to_merge_sends_a_changed_head_back_to_ci() {
    let github = FakeGithub::with_open_pull_request(vec![check("test", None)]);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let report = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "changed");
    assert_eq!(repo.merge_head(run_id).await.expect("read"), None);
}

/// The merge merges the commit the person agreed to; a retry whose merge
/// GitHub took finds it merged and merges nothing again.
#[tokio::test]
async fn merging_again_finds_the_pull_request_merged() {
    let github = FakeGithub::with_open_pull_request(vec![check("test", Some("success"))]);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");

    let first = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");
    let again = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge again");

    assert_eq!(outcome(&first), "merged");
    assert_eq!(outcome(&again), "merged");
    assert_eq!(github.state().merges, [FakeGithub::HEAD]);
}

/// A head that moved after the person agreed is not merged: its checks run
/// again.
#[tokio::test]
async fn a_head_that_moved_since_the_person_agreed_is_not_merged() {
    let github = FakeGithub::with_open_pull_request(vec![check("test", Some("success"))]);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_merge_head(run_id, "2222222222222222222222222222222222222222")
        .await
        .expect("keep");

    let report = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "changed");
    assert!(
        github
            .state()
            .pull_request
            .as_ref()
            .is_some_and(|pull_request| pull_request.state == PullRequestState::Open)
    );
}

/// GitHub refusing the merge ends it `refused` with GitHub's reason; a Run
/// that asked no one to merge merges nothing, for good.
#[tokio::test]
async fn a_refused_merge_or_one_no_person_agreed_to_merges_nothing() {
    let github = FakeGithub::with_open_pull_request(vec![check("test", Some("success"))]);
    github.state().merge_refusal = Some("Required reviews are missing.".to_string());
    let repo = FakeRepo::default();
    let agreed = RunId::generate();
    repo.set_merge_head(agreed, FakeGithub::HEAD)
        .await
        .expect("keep");

    let refused = steps(&github, &repo)
        .merge(input(agreed))
        .await
        .expect("merge");
    let unasked = steps(&github, &repo).merge(input(RunId::generate())).await;

    assert_eq!(outcome(&refused), "refused");
    assert!(refused.output.contains("Required reviews are missing."));
    assert!(matches!(unasked, Err(PullRequestStepError::NoMergeHead)));
}
