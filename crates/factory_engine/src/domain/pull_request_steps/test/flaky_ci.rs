//! A red of only known flaky jobs is rerun once before any CI fix: a rerun
//! that passes is green, one that fails again is red, and a job whose
//! reruns passed often enough is quarantined, with a task filed for it.

use std::collections::VecDeque;

use chrono::Utc;
use factory_records::domain::models::{NewRun, RunId, StepKey};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::quality::{
    CiFlakes, CiRetry, CiRetryMatch, CiRetryResult, FlakyCiFailure,
};
use factory_records::domain::standing_rules::{OrchestratorDefinition, RunRules};

use super::{JULIA, input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::models::{CheckState, FinishedJob};
use crate::domain::pull_request_steps::flaky_ci::flaky_retries;
use crate::domain::service::quarantine::file_tasks;

const FAILED_RUN: u64 = 31;
const RERUN: u64 = 32;

fn job(state: CheckState, step: Option<&str>, actions_job_id: Option<u64>) -> FinishedJob {
    FinishedJob {
        name: "test".to_string(),
        state,
        failed_step: step.map(str::to_string),
        detail: None,
        annotations: Vec::new(),
        actions_job_id,
    }
}

fn failed() -> CheckState {
    CheckState::Failed("failure".to_string())
}

/// A Run kept with `rules`, its Definition naming `test` a known flake.
async fn keep_run(repo: &FakeRepo, rules: RunRules) -> RunId {
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
        standing_rules: rules,
        automation: None,
        definition: None,
        kept_flow: None,
    })
    .await
    .expect("insert");
    repo.keep_known_ci_flakes(run_id, &["test".to_string()])
        .await
        .expect("keep flakes");
    run_id
}

/// GitHub with the pull request red on `test`, an Actions job, which runs
/// again once rerun and ends `rerun_ends`.
fn red_then_rerun(rerun_ends: Option<&str>) -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    let mut state = github.state();
    state.finished = vec![job(failed(), Some("Run e2e"), Some(FAILED_RUN))];
    state.checks_after_rerun = Some(VecDeque::from([ci(None), ci(rerun_ends)]));
    let ended = match rerun_ends {
        Some("success") => CheckState::Passed,
        _ => failed(),
    };
    state.finished_after_rerun = Some(vec![job(ended, Some("Run e2e"), Some(RERUN))]);
    drop(state);
    github
}

fn retries(repo: &FakeRepo, run_id: RunId) -> Vec<CiRetry> {
    repo.ci_retries
        .lock()
        .expect("retries")
        .iter()
        .filter(|(key, _)| key.run_id == run_id)
        .map(|(_, retry)| retry.clone())
        .collect()
}

/// A red of only a known flake reruns it once, and the rerun passing is
/// green: a passed retry, with no CI fix.
#[tokio::test(start_paused = true)]
async fn a_known_flake_that_passes_on_its_rerun_is_green() {
    let github = red_then_rerun(Some("success"));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, RunRules::DEFAULT).await;

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(
        report
            .output
            .contains("`rerun_flaky_ci` rule reran `test` once")
    );
    assert_eq!(github.state().reruns, vec![FAILED_RUN]);
    let kept = retries(&repo, run_id);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].matched, CiRetryMatch::Known);
    assert_eq!(kept[0].result, Some(CiRetryResult::Passed));
    assert!(repo.ci_quarantines.lock().expect("quarantines").is_empty());
}

/// A rerun that fails again goes to the CI fix, and the Step reruns
/// nothing more.
#[tokio::test(start_paused = true)]
async fn a_rerun_that_fails_again_is_red() {
    let github = red_then_rerun(Some("failure"));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, RunRules::DEFAULT).await;

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(report.output.contains("`test` failed again"));
    assert_eq!(github.state().reruns, vec![FAILED_RUN]);
    assert_eq!(
        retries(&repo, run_id)[0].result,
        Some(CiRetryResult::Failed)
    );
}

/// With the rule off, or a red that is no known flake, nothing is rerun.
#[tokio::test(start_paused = true)]
async fn a_red_is_rerun_only_under_the_rule_and_only_for_known_flakes() {
    let off = RunRules {
        definition: OrchestratorDefinition {
            rerun_flaky_ci: false,
            ..OrchestratorDefinition::DEFAULT
        },
        ..RunRules::DEFAULT
    };
    let github = red_then_rerun(Some("success"));
    let repo = FakeRepo::default();
    let ruled_out = keep_run(&repo, off).await;

    let report = steps(&github, &repo)
        .ci(input(ruled_out))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(github.state().reruns.is_empty());

    let github = red_then_rerun(Some("success"));
    let own = keep_run(&repo, RunRules::DEFAULT).await;
    repo.known_ci_flakes.lock().expect("flakes").clear();

    let report = steps(&github, &repo).ci(input(own)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(github.state().reruns.is_empty());
}

/// A job whose reruns passed three times is quarantined once its third
/// passes, and the Run files one task for it as it ends.
#[tokio::test(start_paused = true)]
async fn a_job_whose_reruns_keep_passing_is_quarantined_with_a_task() {
    let github = red_then_rerun(Some("success"));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, RunRules::DEFAULT).await;
    for seq in [1, 2] {
        repo.ci_retries.lock().expect("retries").push((
            StepKey {
                run_id: RunId::generate(),
                seq,
            },
            CiRetry {
                name: "test".to_string(),
                head_sha: FakeGithub::HEAD.to_string(),
                job_id: 7,
                matched: CiRetryMatch::Record,
                result: Some(CiRetryResult::Passed),
                requested_at: Utc::now(),
            },
        ));
    }

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "green");
    assert!(report.output.contains("Quarantined"));
    let quarantines = repo.ci_quarantines.lock().expect("quarantines").clone();
    assert_eq!(quarantines.len(), 1);
    assert_eq!(quarantines[0].name, "test");
    assert_eq!(quarantines[0].repository, "https://github.com/macro/app");
    assert_eq!(quarantines[0].passed_retries, 3);

    let run = repo.get_run(run_id).await.expect("get").expect("kept");
    file_tasks(&repo, &repo, &run).await;
    file_tasks(&repo, &repo, &run).await;

    let follow_ups = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(follow_ups.len(), 1);
    assert_eq!(follow_ups[0].id, quarantines[0].task_id);
    assert!(repo.ci_quarantines.lock().expect("quarantines")[0].filed);
}

/// A failure is taken for flaky by its record only when the same job
/// failed the same way, in a named step, on another pull request; a
/// generic step tells no failure apart, and a job GitHub Actions does not
/// run cannot be rerun.
#[test]
fn a_recorded_failure_is_flaky_only_by_its_identity() {
    let flakes = |step: &str| CiFlakes {
        known: Vec::new(),
        quarantined: Vec::new(),
        failures: vec![FlakyCiFailure {
            name: "test".to_string(),
            failed_step: Some(step.to_string()),
            failure: None,
            pull_requests: 1,
        }],
    };
    let named = [job(failed(), Some("Start the browser"), Some(FAILED_RUN))];

    let retries = flaky_retries(&named, &flakes("Start the browser")).expect("flaky");
    assert_eq!(retries[0].matched, CiRetryMatch::Record);
    assert_eq!(retries[0].job_id, FAILED_RUN);
    assert_eq!(flaky_retries(&named, &flakes("Seed the database")), None);
    let generic = [job(failed(), Some("Run tests"), Some(FAILED_RUN))];
    assert_eq!(flaky_retries(&generic, &flakes("Run tests")), None);
    let not_actions = [job(failed(), Some("Start the browser"), None)];
    assert_eq!(
        flaky_retries(&not_actions, &flakes("Start the browser")),
        None
    );
}
