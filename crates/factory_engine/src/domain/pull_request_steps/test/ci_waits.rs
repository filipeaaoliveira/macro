//! Each wait of `ci` is kept with the jobs that finished on its head, for
//! CI health; keeping it never holds the Run up.

use factory_records::domain::models::{RunId, StepKey};
use factory_records::domain::quality::{CiJob, CiJobResult, CiVerdict};

use super::{input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::models::{CheckState, FinishedJob};

fn finished(name: &str, state: CheckState, failed_step: Option<&str>) -> FinishedJob {
    FinishedJob {
        name: name.to_string(),
        state,
        failed_step: failed_step.map(str::to_string),
        detail: None,
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
            },
            CiJob {
                name: "build".to_string(),
                result: CiJobResult::Setup,
                failed_step: Some("Set up job".to_string()),
            },
            CiJob {
                name: "e2e".to_string(),
                result: CiJobResult::Cancelled,
                failed_step: None,
            },
            CiJob {
                name: "lint".to_string(),
                result: CiJobResult::Passed,
                failed_step: None,
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
