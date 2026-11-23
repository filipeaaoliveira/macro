//! Each wait of `ci` is kept with the jobs that finished on its head, for
//! CI health; keeping it never holds the Run up.

use factory_records::domain::models::{RunId, StepKey};
use factory_records::domain::quality::{CiJob, CiJobResult, CiVerdict};

use super::{input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::models::{CheckState, FinishedJob};
use crate::domain::pull_request_steps::CI_RECORD_BOUND;

fn finished(name: &str, state: CheckState, failed_step: Option<&str>) -> FinishedJob {
    FinishedJob {
        name: name.to_string(),
        state,
        failed_step: failed_step.map(str::to_string),
        detail: None,
        annotations: Vec::new(),
        actions_job_id: None,
    }
}

fn failed(conclusion: &str) -> CheckState {
    CheckState::Failed(conclusion.to_string())
}

/// A red wait keeps the jobs that finished on its head, each classed: a
/// test step that failed is the job's own failure, a step that sets the
/// job up is the CI's, and a cancelled run is cancelled.
#[tokio::test(start_paused = true)]
async fn a_red_wait_is_kept_with_its_jobs_classed() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    github.state().finished = vec![
        finished("test", failed("failure"), Some("Run tests")),
        finished("build", failed("failure"), Some("Set up job")),
        finished("e2e", failed("cancelled"), None),
        finished("lint", CheckState::Passed, None),
    ];
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    let waits = repo.ci_waits.lock().expect("waits").clone();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].key, StepKey { run_id, seq: 5 });
    assert_eq!(waits[0].head_sha, FakeGithub::HEAD);
    assert_eq!(waits[0].verdict, CiVerdict::Red);
    assert_eq!(
        waits[0].jobs,
        vec![
            CiJob {
                name: "test".to_string(),
                result: CiJobResult::Failed,
                failed_step: Some("Run tests".to_string()),
                failure: None,
                replaced: false,
            },
            CiJob {
                name: "build".to_string(),
                result: CiJobResult::Setup,
                failed_step: Some("Set up job".to_string()),
                failure: None,
                replaced: false,
            },
            CiJob {
                name: "e2e".to_string(),
                result: CiJobResult::Cancelled,
                failed_step: None,
                failure: None,
                replaced: false,
            },
            CiJob {
                name: "lint".to_string(),
                result: CiJobResult::Passed,
                failed_step: None,
                failure: None,
                replaced: false,
            },
        ]
    );
    assert_eq!(github.state().finished_reads, vec![FakeGithub::HEAD]);
}

/// A green wait is kept too; one that timed out keeps no jobs, which had
/// not finished, and reads none.
#[tokio::test(start_paused = true)]
async fn green_and_timed_out_waits_are_kept() {
    let green = FakeGithub::with_open_pull_request(ci(Some("success")));
    green.state().finished = vec![finished("test", CheckState::Passed, None)];
    let slow = FakeGithub::with_open_pull_request(ci(None));
    let repo = FakeRepo::default();
    let (green_run, slow_run) = (RunId::generate(), RunId::generate());

    steps(&green, &repo).ci(input(green_run)).await.expect("ci");
    let timed_out = steps(&slow, &repo).ci(input(slow_run)).await.expect("ci");

    assert_eq!(outcome(&timed_out), "timeout");
    let waits = repo.ci_waits.lock().expect("waits").clone();
    let verdicts: Vec<_> = waits
        .iter()
        .map(|wait| (wait.key.run_id, wait.verdict, wait.jobs.len()))
        .collect();
    assert_eq!(
        verdicts,
        vec![
            (green_run, CiVerdict::Green, 1),
            (slow_run, CiVerdict::Timeout, 0)
        ]
    );
    assert!(slow.state().finished_reads.is_empty());
}

/// Jobs that cannot be read leave the wait kept without them, and a wait
/// the records cannot keep leaves the Step's verdict as it was.
#[tokio::test(start_paused = true)]
async fn keeping_a_wait_never_holds_the_run_up() {
    let unread = FakeGithub::with_open_pull_request(ci(Some("failure")));
    unread.state().finished_fails = true;
    let repo = FakeRepo::default();
    let refusing = FakeRepo::default();
    refusing
        .failing
        .lock()
        .expect("failing")
        .insert("record_ci_wait", (0, 1));
    let unkept = FakeGithub::with_open_pull_request(ci(Some("failure")));

    let red = steps(&unread, &repo)
        .ci(input(RunId::generate()))
        .await
        .expect("ci");
    let still_red = steps(&unkept, &refusing)
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&red), "red");
    let waits = repo.ci_waits.lock().expect("waits").clone();
    assert_eq!(waits.len(), 1);
    assert!(waits[0].jobs.is_empty());
    assert_eq!(outcome(&still_red), "red");
    assert!(refusing.ci_waits.lock().expect("waits").is_empty());
}

const REPLACED: &str = "2222222222222222222222222222222222222222";

fn cancelled_job(name: &str) -> CiJob {
    CiJob {
        name: name.to_string(),
        result: CiJobResult::Cancelled,
        failed_step: None,
        failure: None,
        replaced: true,
    }
}

/// A GitHub that answers slowly holds the verdict up no longer than the
/// keeping's bound: the wait is kept without its jobs, and the Step routes
/// the verdict it read.
#[tokio::test(start_paused = true)]
async fn a_slow_github_never_holds_the_verdict_past_the_bound() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    github.state().finished = vec![finished("test", failed("failure"), Some("Run tests"))];
    github.state().finished_delay = Some(std::time::Duration::from_secs(10 * 60));
    let repo = FakeRepo::default();
    let started = tokio::time::Instant::now();

    let report = steps(&github, &repo)
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(
        started.elapsed() <= CI_RECORD_BOUND,
        "the verdict waited {:?}",
        started.elapsed()
    );
    let waits = repo.ci_waits.lock().expect("waits").clone();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].verdict, CiVerdict::Red);
    assert!(waits[0].jobs.is_empty());
}

/// A wait that ends near its deadline reads its jobs without their steps
/// and annotations; one that ends early reads them.
#[tokio::test(start_paused = true)]
async fn a_wait_near_its_deadline_reads_no_details() {
    let late = FakeGithub::with_open_pull_request(ci(None));
    // Pending on every read but the last before the deadline.
    let mut checks: std::collections::VecDeque<_> = (0..75).map(|_| ci(None)).collect();
    checks.push_back(ci(Some("failure")));
    late.state().checks = checks;
    let early = FakeGithub::with_open_pull_request(ci(Some("failure")));
    let repo = FakeRepo::default();

    let late_report = steps(&late, &repo)
        .ci(input(RunId::generate()))
        .await
        .expect("ci");
    steps(&early, &repo)
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&late_report), "red");
    assert_eq!(late.state().finished_details, vec![false]);
    assert_eq!(early.state().finished_details, vec![true]);
}

/// A push during the wait replaces the head: the runs it cancelled on the
/// head it replaced are kept, marked replaced, beside the new head's jobs;
/// that head's runs that finished otherwise are not.
#[tokio::test(start_paused = true)]
async fn runs_a_push_during_the_wait_cancelled_are_kept() {
    let github = FakeGithub::with_open_pull_request(ci(None));
    github.state().checks = [ci(None), ci(Some("failure"))].into();
    github.state().heads = [REPLACED.to_string(), FakeGithub::HEAD.to_string()].into();
    github.state().finished_on = [
        (
            REPLACED.to_string(),
            vec![
                finished("test", failed("cancelled"), None),
                finished("lint", CheckState::Passed, None),
            ],
        ),
        (
            FakeGithub::HEAD.to_string(),
            vec![finished("test", failed("failure"), Some("Run tests"))],
        ),
    ]
    .into();
    let repo = FakeRepo::default();

    let report = steps(&github, &repo)
        .ci(input(RunId::generate()))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    let waits = repo.ci_waits.lock().expect("waits").clone();
    assert_eq!(waits[0].head_sha, FakeGithub::HEAD);
    assert_eq!(
        waits[0].jobs,
        vec![
            CiJob {
                name: "test".to_string(),
                result: CiJobResult::Failed,
                failed_step: Some("Run tests".to_string()),
                failure: None,
                replaced: false,
            },
            cancelled_job("test"),
        ]
    );
    // The replaced head is read once, without details.
    assert_eq!(
        github.state().finished_reads,
        vec![FakeGithub::HEAD.to_string(), REPLACED.to_string()]
    );
    assert_eq!(github.state().finished_details, vec![true, false]);
}

/// A push between two waits, as a fix's, replaces the head the last wait
/// read: the runs it cancelled there are kept with the next wait.
#[tokio::test(start_paused = true)]
async fn runs_a_push_since_the_last_wait_cancelled_are_kept() {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    github.state().finished_on = [(
        REPLACED.to_string(),
        vec![
            finished("e2e", failed("cancelled"), None),
            finished("test", failed("failure"), Some("Run tests")),
        ],
    )]
    .into();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    repo.ci_waits
        .lock()
        .expect("waits")
        .push(factory_records::domain::quality::CiWait {
            key: StepKey { run_id, seq: 3 },
            head_sha: REPLACED.to_string(),
            verdict: CiVerdict::Red,
            jobs: Vec::new(),
        });

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "green");
    let waits = repo.ci_waits.lock().expect("waits").clone();
    let kept = waits
        .iter()
        .find(|wait| wait.key.seq == 5)
        .expect("this wait");
    assert_eq!(kept.verdict, CiVerdict::Green);
    assert!(kept.jobs.contains(&cancelled_job("e2e")));
    assert!(
        !kept
            .jobs
            .iter()
            .any(|job| job.replaced && job.name == "test")
    );
}

/// A retry of the Step that reads green on the head an earlier attempt
/// called red keeps the green: the verdict the Step routes.
#[tokio::test(start_paused = true)]
async fn a_retry_that_reads_green_keeps_green() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    steps(&github, &repo).ci(input(run_id)).await.expect("ci");
    github.state().checks = [ci(Some("success"))].into();
    let retried = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&retried), "green");
    let waits = repo.ci_waits.lock().expect("waits").clone();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].verdict, CiVerdict::Green);
}

/// A job the last wait already kept on its head, as a sibling a matrix's
/// fail-fast cancelled, finished before the push: the next wait does not
/// keep it again as cancelled by that push.
#[tokio::test(start_paused = true)]
async fn runs_the_last_wait_kept_are_not_blamed_on_the_push() {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    github.state().finished_on = [(
        REPLACED.to_string(),
        vec![
            finished("test (1)", failed("failure"), Some("Run tests")),
            finished("test (2)", failed("cancelled"), None),
            finished("e2e", failed("cancelled"), None),
        ],
    )]
    .into();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let kept_before = |name: &str, result| CiJob {
        name: name.to_string(),
        result,
        failed_step: None,
        failure: None,
        replaced: false,
    };
    repo.ci_waits
        .lock()
        .expect("waits")
        .push(factory_records::domain::quality::CiWait {
            key: StepKey { run_id, seq: 3 },
            head_sha: REPLACED.to_string(),
            verdict: CiVerdict::Red,
            jobs: vec![
                kept_before("test (1)", CiJobResult::Failed),
                kept_before("test (2)", CiJobResult::Cancelled),
            ],
        });

    steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    let waits = repo.ci_waits.lock().expect("waits").clone();
    let kept = waits
        .iter()
        .find(|wait| wait.key.seq == 5)
        .expect("this wait");
    let replaced: Vec<_> = kept
        .jobs
        .iter()
        .filter(|job| job.replaced)
        .map(|job| job.name.as_str())
        .collect();
    // Only the run still going when the last wait ended was the push's.
    assert_eq!(replaced, vec!["e2e"]);
}
