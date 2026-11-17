use std::sync::LazyLock;

use chrono::Utc;

use factory_records::domain::models::{Repository, RunId, StepKey};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{CI_WAIT, PullRequestStepError, PullRequestSteps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci, suite};
use crate::domain::models::{
    Check, CheckState, CheckSuite, CommitStatus, HeadChecks, PullRequestState, RequiredCheck,
    RequiredChecks, StepInput, StepReport,
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
            state: CheckState::of_status("failure"),
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

/// Checks that passed on two reads in a row are green, and the head they
/// passed on is kept as the one a person may merge; an app's suite that
/// stayed empty past the grace, as one that never runs anything, is not
/// waited for.
#[tokio::test(start_paused = true)]
async fn ci_is_green_once_two_reads_see_every_check_passed() {
    let mut passing = ci(Some("success"));
    passing.suites.push(CheckSuite {
        check_runs: 0,
        created_at: Some(Utc::now() - chrono::TimeDelta::minutes(10)),
        ..suite(9, "other-app", None)
    });
    let github = FakeGithub::with_open_pull_request(HeadChecks::default());
    github.state().checks = [ci(None), passing].into();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(report.output.contains(FakeGithub::HEAD));
    assert_eq!(github.state().reads, 3);
    assert_eq!(
        repo.merge_head(run_id).await.expect("read").as_deref(),
        Some(FakeGithub::HEAD)
    );
}

fn required(name: &str, app_id: Option<u64>) -> RequiredCheck {
    RequiredCheck {
        name: name.to_string(),
        app_id,
    }
}

/// An empty suite of an app with no run waiting on the head holds CI back
/// within its grace, as one that adds its runs late, and is let go once the
/// grace ends, within the same Step.
#[tokio::test(start_paused = true)]
async fn another_app_s_empty_suite_is_let_go_once_its_grace_ends() {
    let mut early = ci(Some("success"));
    early.suites.push(CheckSuite {
        check_runs: 0,
        ..suite(9, "another-app", None)
    });
    let github = FakeGithub::with_open_pull_request(early);
    let started = tokio::time::Instant::now();

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "green");
    let waited = started.elapsed();
    assert!(
        waited >= std::time::Duration::from_secs(5 * 60),
        "{waited:?}"
    );
    assert!(waited < CI_WAIT);
}

/// With no required checks to go by, the empty suite of an app with a run
/// waiting on the head, as a GitHub Actions run a concurrency group holds,
/// is waited for past the grace, until CI times out.
#[tokio::test(start_paused = true)]
async fn a_held_run_s_empty_suite_is_waited_for_until_ci_times_out() {
    let mut held = ci(Some("success"));
    held.suites.push(CheckSuite {
        check_runs: 0,
        ..suite(9, "held-ci", None)
    });
    held.held_apps = vec!["held-ci".to_string()];
    let github = FakeGithub::with_open_pull_request(held);

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "timeout");
}

/// An Actions run that has not started checking holds CI though every suite
/// reported passed, as one whose suite GitHub has not made yet.
#[tokio::test(start_paused = true)]
async fn a_held_run_with_no_suite_yet_holds_ci() {
    let github = FakeGithub::with_open_pull_request(HeadChecks {
        held_apps: vec!["github-actions".to_string()],
        ..ci(Some("success"))
    });

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "timeout");
}

/// Once every check the base requires, read in full, appeared and passed,
/// other apps' empty suites, even a held run's, do not hold CI back: green
/// on the next read.
#[tokio::test(start_paused = true)]
async fn required_checks_that_passed_let_empty_suites_go() {
    let mut passed = ci(Some("success"));
    for (id, app) in [(8, "other-app"), (9, "held-ci")] {
        passed.suites.push(CheckSuite {
            check_runs: 0,
            ..suite(id, app, None)
        });
    }
    passed.held_apps = vec!["held-ci".to_string()];
    let github = FakeGithub::with_open_pull_request(passed);
    github.state().required = Some(RequiredChecks {
        checks: vec![required("test", Some(FakeGithub::ACTIONS))],
        complete: true,
    });
    let started = tokio::time::Instant::now();

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(started.elapsed() <= std::time::Duration::from_secs(20));
}

/// A required set read only in part must pass, but may leave a required
/// check out, so another app's empty suite still holds CI for its grace.
#[tokio::test(start_paused = true)]
async fn a_partly_read_required_set_lets_no_empty_suite_go_early() {
    let mut passed = ci(Some("success"));
    passed.suites.push(CheckSuite {
        check_runs: 0,
        ..suite(8, "other-app", None)
    });
    let github = FakeGithub::with_open_pull_request(passed);
    github.state().required = Some(RequiredChecks {
        checks: vec![required("test", Some(FakeGithub::ACTIONS))],
        complete: false,
    });
    let started = tokio::time::Instant::now();

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(started.elapsed() >= std::time::Duration::from_secs(5 * 60));
}

/// A required check is waited for while it has not appeared, while its run
/// still runs, while only another app's run of that name passed, or while
/// only a commit status of that name passed but the check names an app.
#[tokio::test(start_paused = true)]
async fn a_required_check_is_waited_for_until_its_own_run_passed() {
    let missing = FakeGithub::with_open_pull_request(ci(Some("success")));
    missing.state().required = Some(RequiredChecks {
        checks: vec![required("deploy", None)],
        complete: true,
    });
    let mut running_checks = ci(Some("success"));
    running_checks.runs.push(Check {
        name: "deploy".to_string(),
        state: CheckState::Waiting,
        url: None,
        app_id: Some(FakeGithub::ACTIONS),
    });
    let running = FakeGithub::with_open_pull_request(running_checks);
    running.state().required = Some(RequiredChecks {
        checks: vec![required("deploy", None)],
        complete: true,
    });
    let other_app = FakeGithub::with_open_pull_request(ci(Some("success")));
    other_app.state().required = Some(RequiredChecks {
        checks: vec![required("test", Some(42))],
        complete: true,
    });
    let status_only = FakeGithub::with_open_pull_request(HeadChecks {
        statuses: vec![CommitStatus {
            context: "deploy".to_string(),
            state: CheckState::Passed,
            url: None,
        }],
        ..ci(Some("success"))
    });
    status_only.state().required = Some(RequiredChecks {
        checks: vec![required("deploy", Some(42))],
        complete: true,
    });

    for github in [missing, running, other_app, status_only] {
        let report = steps(&github, &FakeRepo::default())
            .ci(input(RunId::generate()))
            .await
            .expect("ci");
        assert_eq!(outcome(&report), "timeout");
    }
}

/// A required check with no app passes as a commit status of its name.
#[tokio::test(start_paused = true)]
async fn a_required_check_with_no_app_passes_as_a_commit_status() {
    let github = FakeGithub::with_open_pull_request(HeadChecks {
        statuses: vec![CommitStatus {
            context: "deploy".to_string(),
            state: CheckState::Passed,
            url: None,
        }],
        ..ci(Some("success"))
    });
    github.state().required = Some(RequiredChecks {
        checks: vec![required("deploy", None), required("test", None)],
        complete: true,
    });

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "green");
}

/// Checks read from a list longer than was read are not all there is: CI
/// waits rather than calling them green.
#[tokio::test(start_paused = true)]
async fn checks_from_a_list_longer_than_was_read_are_waited_for() {
    let github = FakeGithub::with_open_pull_request(HeadChecks {
        truncated: true,
        ..ci(Some("success"))
    });

    let report = steps(&github, &FakeRepo::default())
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "timeout");
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

/// The head CI called green is the one a person is asked to merge.
#[tokio::test]
async fn ready_to_merge_asks_about_the_head_ci_called_green() {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");

    let report = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "done");
    assert!(report.output.contains(FakeGithub::HEAD));
}

/// A head that is not the one CI called green, though its checks pass, as
/// after a push whose checks are fast, goes back to CI rather than to a
/// person; so does a head whose checks no longer pass.
#[tokio::test]
async fn ready_to_merge_sends_a_changed_head_back_to_ci() {
    let pushed = FakeGithub::with_open_pull_request(ci(Some("success")));
    let running = FakeGithub::with_open_pull_request(ci(None));
    let repo = FakeRepo::default();
    let moved = RunId::generate();
    repo.set_merge_head(moved, "2222222222222222222222222222222222222222")
        .await
        .expect("keep");
    let rerun = RunId::generate();
    repo.set_merge_head(rerun, FakeGithub::HEAD)
        .await
        .expect("keep");

    let after_push = steps(&pushed, &repo)
        .ready_to_merge(input(moved))
        .await
        .expect("ready");
    let checks_running = steps(&running, &repo)
        .ready_to_merge(input(rerun))
        .await
        .expect("ready");

    assert_eq!(outcome(&after_push), "changed");
    assert_eq!(outcome(&checks_running), "changed");
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
    // Characters, not bytes, count: each of these is two bytes.
    let request = "é".repeat(70_000);
    let mut long = input(RunId::generate());
    long.request = &request;

    steps(&github, &FakeRepo::default())
        .publish(long)
        .await
        .expect("publish");

    let body = github.state().bodies[0].clone();
    assert!(body.chars().count() <= 65_536);
    assert!(body.starts_with("ééé"));
    assert!(body.contains("(The rest of the request is left out.)"));
    assert!(body.contains("\n\nTask: MACRO-"));
}
