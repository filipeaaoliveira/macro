use std::sync::LazyLock;

use factory_records::domain::models::{Repository, RunId, StepKey};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{CI_WAIT, PullRequestStepError, PullRequestSteps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci, suite};
use crate::domain::models::{
    CheckSuite, CommitStatus, HeadChecks, PullRequestState, StepInput, StepReport,
};

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
    let opened_elsewhere = FakeGithub::with_open_pull_request(HeadChecks::default());
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
    let github = FakeGithub::with_open_pull_request(HeadChecks::default());
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

/// CI ends `red` as soon as a suite fails, naming the runs that failed in
/// it.
#[tokio::test(start_paused = true)]
async fn ci_is_red_with_the_checks_that_failed() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(report.output.contains("- github-actions checks (failure)"));
    assert!(
        report
            .output
            .contains("- test (failure): https://github.com/macro/app/runs/test")
    );
}

/// A commit status that failed is red, though every check suite passed.
#[tokio::test(start_paused = true)]
async fn a_failing_commit_status_is_red_beside_passing_checks() {
    let github = FakeGithub::with_open_pull_request(HeadChecks {
        statuses: vec![CommitStatus {
            context: "ci/lint".to_string(),
            state: "failure".to_string(),
            url: Some("https://ci.example.com/1".to_string()),
        }],
        ..ci(Some("success"))
    });

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(
        report
            .output
            .contains("- ci/lint (failure): https://ci.example.com/1")
    );
}

/// CI waits while checks run or before any is reported, and ends `green`
/// only once two reads in a row see the same checks all passed: a check
/// that appears after the first ones passed, such as a job that waits on
/// another, is waited for, and its failure is red.
#[tokio::test(start_paused = true)]
async fn a_check_that_appears_after_the_others_passed_is_waited_for() {
    let github = FakeGithub::with_open_pull_request(HeadChecks::default());
    let late = HeadChecks {
        suites: vec![
            suite(1, "github-actions", Some("success")),
            suite(2, "deploy-preview", Some("failure")),
        ],
        ..HeadChecks::default()
    };
    github.state().checks = [HeadChecks::default(), ci(None), ci(Some("success")), late].into();

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(report.output.contains("deploy-preview checks (failure)"));
}

/// Checks that passed on two reads in a row are green; an app's empty
/// suite, which never runs anything, is not waited for.
#[tokio::test(start_paused = true)]
async fn ci_is_green_once_two_reads_see_every_check_passed() {
    let mut passing = ci(Some("success"));
    passing.suites.push(CheckSuite {
        check_runs: 0,
        ..suite(9, "other-app", None)
    });
    let github = FakeGithub::with_open_pull_request(HeadChecks::default());
    github.state().checks = [ci(None), passing].into();

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(report.output.contains(FakeGithub::HEAD));
    assert_eq!(github.state().reads, 3);
}

/// Checks that never finish end CI `timeout` once it waited [`CI_WAIT`].
#[tokio::test(start_paused = true)]
async fn ci_that_never_finishes_times_out() {
    let github = FakeGithub::with_open_pull_request(ci(None));
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
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
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
    let github = FakeGithub::with_open_pull_request(ci(None));
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
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
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
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
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
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
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

/// A request too long for GitHub's description is cut, and the lines that
/// name the Run and its task stay.
#[tokio::test]
async fn a_long_request_is_cut_to_fit_the_description_keeping_the_task() {
    let github = FakeGithub::default();
    let request = "x".repeat(70_000);
    let mut long = input(RunId::generate());
    long.request = &request;

    steps(&github, &FakeRepo::default())
        .publish(long)
        .await
        .expect("publish");

    let body = github.state().bodies[0].clone();
    assert!(body.chars().count() <= 65_536);
    assert!(body.contains("(The rest of the request is left out.)"));
    assert!(body.contains("\n\nTask: MACRO-"));
}
