//! A red of only known flaky jobs is rerun once before any CI fix: a rerun
//! that passes is green, one that fails again, never shows, is declined or
//! still runs at the deadline is red, and a failure whose reruns passed
//! often enough is quarantined, with a task filed for it.

use std::collections::VecDeque;

use chrono::Utc;
use factory_records::domain::models::{NewRun, RunId, StepKey};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::quality::{
    CiFailureIdentity, CiFlakes, CiRetry, CiRetryMatch, CiRetryResult, FlakyCiFailure,
};
use factory_records::domain::standing_rules::{OrchestratorDefinition, RunRules};

use super::{JULIA, input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci, suite};
use crate::domain::models::{Check, CheckState, FinishedJob, HeadChecks, JobRerun};
use crate::domain::pull_request_steps::CI_WAIT;
use crate::domain::pull_request_steps::flaky_ci::{
    HOLD_PAST_RUN, RERUN_SHOWS_WITHIN, flaky_retries, may_be_flaky,
};
use crate::domain::service::quarantine::file_tasks;

const FAILED_RUN: u64 = 31;
const RERUN: u64 = 32;
const LINT_RUN: u64 = 41;
const WORKFLOW: u64 = 900;

fn named_job(name: &str, state: CheckState, step: Option<&str>, id: Option<u64>) -> FinishedJob {
    FinishedJob {
        name: name.to_string(),
        state,
        failed_step: step.map(str::to_string),
        detail: None,
        annotations: Vec::new(),
        actions_job_id: id,
        workflow_run_id: id.map(|_| WORKFLOW),
    }
}

fn job(state: CheckState, step: Option<&str>, id: Option<u64>) -> FinishedJob {
    named_job("test", state, step, id)
}

fn failed() -> CheckState {
    CheckState::Failed("failure".to_string())
}

/// The rules with `rerun_flaky_ci` on.
fn rule_on() -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            rerun_flaky_ci: true,
            ..OrchestratorDefinition::DEFAULT
        },
        ..RunRules::DEFAULT
    }
}

/// A Run kept with `rules`, its Definition naming `test` and `lint` known
/// flakes.
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
    repo.keep_known_ci_flakes(run_id, &["test".to_string(), "lint".to_string()])
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

fn results(repo: &FakeRepo, run_id: RunId) -> Vec<(String, Option<CiRetryResult>)> {
    retries(repo, run_id)
        .into_iter()
        .map(|retry| (retry.name, retry.result))
        .collect()
}

/// A red of only a known flake reruns it once, and the rerun passing is
/// green: a passed retry, with no CI fix. The evidence is read once.
#[tokio::test(start_paused = true)]
async fn a_known_flake_that_passes_on_its_rerun_is_green() {
    let github = red_then_rerun(Some("success"));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;

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
    assert_eq!(kept[0].workflow_run_id, Some(WORKFLOW));
    assert_eq!(kept[0].result, Some(CiRetryResult::Passed));
    assert!(repo.ci_quarantines.lock().expect("quarantines").is_empty());
    assert_eq!(repo.ci_flakes_reads.lock().expect("reads").len(), 1);
}

/// A rerun that fails again goes to the CI fix.
#[tokio::test(start_paused = true)]
async fn a_rerun_that_fails_again_is_red() {
    let github = red_then_rerun(Some("failure"));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(report.output.contains("`test` failed again"));
    assert_eq!(github.state().reruns, vec![FAILED_RUN]);
    assert_eq!(
        results(&repo, run_id),
        vec![("test".to_string(), Some(CiRetryResult::Failed))]
    );
}

/// A rerun of the same name in another workflow run is not the job's
/// rerun: the job's own rerun decides.
#[tokio::test(start_paused = true)]
async fn a_rerun_is_told_by_its_workflow_run() {
    let github = red_then_rerun(Some("failure"));
    let mut elsewhere = job(CheckState::Passed, None, Some(77));
    elsewhere.workflow_run_id = Some(WORKFLOW + 1);
    github.state().finished_after_rerun =
        Some(vec![elsewhere, job(failed(), Some("Run e2e"), Some(RERUN))]);
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    assert_eq!(
        results(&repo, run_id),
        vec![("test".to_string(), Some(CiRetryResult::Failed))]
    );
}

/// A rerun GitHub never shows reads as the run that failed: once
/// [`RERUN_SHOWS_WITHIN`] passed, it is red.
#[tokio::test(start_paused = true)]
async fn a_rerun_that_never_shows_is_red_after_its_grace() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    github.state().finished = vec![job(failed(), Some("Run e2e"), Some(FAILED_RUN))];
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;
    let started = tokio::time::Instant::now();

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    let waited = chrono::TimeDelta::from_std(started.elapsed()).expect("time");
    assert!(waited > RERUN_SHOWS_WITHIN, "waited {waited}");
    assert!(started.elapsed() < CI_WAIT);
    assert_eq!(
        results(&repo, run_id),
        vec![("test".to_string(), Some(CiRetryResult::Failed))]
    );
}

/// A rerun still running when the wait ends is the red it was asked for:
/// it is settled, and the Step goes to the fix rather than timing out.
#[tokio::test(start_paused = true)]
async fn a_rerun_running_at_the_deadline_is_red() {
    let github = FakeGithub::with_open_pull_request(ci(Some("failure")));
    github.state().finished = vec![job(failed(), Some("Run e2e"), Some(FAILED_RUN))];
    github.state().checks_after_rerun = Some(VecDeque::from([ci(None)]));
    github.state().finished_after_rerun = Some(Vec::new());
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(report.output.contains("did not finish"));
    assert_eq!(
        results(&repo, run_id),
        vec![("test".to_string(), Some(CiRetryResult::Failed))]
    );
}

/// A red read too near the deadline for a rerun to finish is not rerun.
#[tokio::test(start_paused = true)]
async fn a_red_too_near_the_deadline_is_not_rerun() {
    let github = red_then_rerun(Some("success"));
    let mut checks: VecDeque<HeadChecks> = (0..60).map(|_| ci(None)).collect();
    checks.push_back(ci(Some("failure")));
    github.state().checks = checks;
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(report.output.contains("too little of the wait"));
    assert!(github.state().reruns.is_empty());
}

/// The red of `test` and `lint`, both known flakes.
fn two_red_jobs() -> FakeGithub {
    let mut checks = ci(Some("failure"));
    checks.runs.push(Check {
        name: "lint".to_string(),
        state: failed(),
        url: None,
        app_id: Some(FakeGithub::ACTIONS),
    });
    let github = FakeGithub::with_open_pull_request(checks);
    github.state().finished = vec![
        job(failed(), Some("Run e2e"), Some(FAILED_RUN)),
        named_job("lint", failed(), Some("Lint docs"), Some(LINT_RUN)),
    ];
    github
}

/// A rerun GitHub declines, or one that cannot be asked, is refused, and
/// the red goes to the fix; a partly declined rerun settles every rerun
/// the attempt kept.
#[tokio::test(start_paused = true)]
async fn a_declined_rerun_is_refused_and_red() {
    let declined = red_then_rerun(Some("success"));
    declined.state().rerun_answers.insert(
        FAILED_RUN,
        JobRerun::Declined("Too old to rerun".to_string()),
    );
    let unasked = red_then_rerun(Some("success"));
    unasked.state().rerun_fails.insert(FAILED_RUN);
    let partly = two_red_jobs();
    partly
        .state()
        .rerun_answers
        .insert(LINT_RUN, JobRerun::Declined("Not allowed".to_string()));
    let repo = FakeRepo::default();

    for github in [&declined, &unasked, &partly] {
        let run_id = keep_run(&repo, rule_on()).await;

        let report = steps(github, &repo).ci(input(run_id)).await.expect("ci");

        assert_eq!(outcome(&report), "red");
        assert!(report.output.contains("GitHub would not rerun"));
        assert!(
            results(&repo, run_id)
                .iter()
                .all(|(_, result)| *result == Some(CiRetryResult::Refused))
        );
    }
    assert_eq!(partly.state().reruns, vec![FAILED_RUN, LINT_RUN]);
}

/// Two attempts of the Step at once rerun the job once: only the attempt
/// that kept the rerun asks GitHub.
#[tokio::test(start_paused = true)]
async fn two_attempts_at_once_rerun_a_job_once() {
    let github = red_then_rerun(Some("success"));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;

    let (first, second) = (steps(&github, &repo), steps(&github, &repo));
    let (one, other) = tokio::join!(first.ci(input(run_id)), second.ci(input(run_id)));

    assert_eq!(outcome(&one.expect("one")), "green");
    assert_eq!(outcome(&other.expect("other")), "green");
    assert_eq!(github.state().reruns, vec![FAILED_RUN]);
}

/// A red is held while other checks of the head run, and rerun once they
/// finish; a hold lasts at most [`HOLD_PAST_RUN`] once the failed jobs'
/// workflow runs completed.
#[tokio::test(start_paused = true)]
async fn a_red_is_held_while_other_checks_run() {
    let mut running = ci(Some("failure"));
    running.suites.push(suite(2, "github-actions", None));
    let finishing = red_then_rerun(Some("success"));
    finishing.state().checks =
        VecDeque::from([running.clone(), running.clone(), ci(Some("failure"))]);
    let never_finishing = red_then_rerun(Some("success"));
    never_finishing.state().checks = VecDeque::from([running]);
    let repo = FakeRepo::default();

    let finished_run = keep_run(&repo, rule_on()).await;
    let report = steps(&finishing, &repo)
        .ci(input(finished_run))
        .await
        .expect("ci");
    assert_eq!(outcome(&report), "green");
    assert_eq!(finishing.state().reruns, vec![FAILED_RUN]);
    // Held twice, then read once more before the rerun.
    assert!(finishing.state().reads >= 3);

    let capped_run = keep_run(&repo, rule_on()).await;
    let started = tokio::time::Instant::now();
    let report = steps(&never_finishing, &repo)
        .ci(input(capped_run))
        .await
        .expect("ci");
    assert_eq!(outcome(&report), "green");
    assert_eq!(never_finishing.state().reruns, vec![FAILED_RUN]);
    assert!(started.elapsed() >= HOLD_PAST_RUN);
    assert_eq!(repo.ci_flakes_reads.lock().expect("reads").len(), 2);
}

/// With the rule off, or a red that is no known flake, nothing is rerun.
#[tokio::test(start_paused = true)]
async fn a_red_is_rerun_only_under_the_rule_and_only_for_known_flakes() {
    let github = red_then_rerun(Some("success"));
    let repo = FakeRepo::default();
    let ruled_out = keep_run(&repo, RunRules::DEFAULT).await;

    let report = steps(&github, &repo)
        .ci(input(ruled_out))
        .await
        .expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(github.state().reruns.is_empty());

    let github = red_then_rerun(Some("success"));
    let own = keep_run(&repo, rule_on()).await;
    repo.known_ci_flakes.lock().expect("flakes").clear();

    let report = steps(&github, &repo).ci(input(own)).await.expect("ci");

    assert_eq!(outcome(&report), "red");
    assert!(github.state().reruns.is_empty());
}

/// A failure whose reruns passed three times is quarantined once its third
/// passes, and the Run files one task for it as it ends.
#[tokio::test(start_paused = true)]
async fn a_failure_whose_reruns_keep_passing_is_quarantined_with_a_task() {
    let github = red_then_rerun(Some("success"));
    let repo = FakeRepo::default();
    let run_id = keep_run(&repo, rule_on()).await;
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
                workflow_run_id: None,
                failed_step: Some("Run e2e".to_string()),
                failure: None,
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
    assert_eq!(
        quarantines[0].failure,
        CiFailureIdentity {
            name: "test".to_string(),
            failed_step: Some("Run e2e".to_string()),
            failure: None,
        }
    );
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

fn proved(step: &str, pull_requests: u32) -> CiFlakes {
    CiFlakes {
        known: Vec::new(),
        quarantined: Vec::new(),
        failures: vec![FlakyCiFailure {
            name: "test".to_string(),
            failed_step: Some(step.to_string()),
            failure: None,
            pull_requests,
        }],
    }
}

/// A failure is taken for flaky by its record only when the same job
/// proved flaky failing the same way, in a named step, on two other pull
/// requests; a generic step tells no failure apart, and a job GitHub
/// Actions does not run cannot be rerun, nor two failed jobs of one name.
#[test]
fn a_recorded_failure_is_flaky_only_by_its_identity() {
    let named = [job(failed(), Some("Start the browser"), Some(FAILED_RUN))];

    let retries = flaky_retries(&named, &proved("Start the browser", 2)).expect("flaky");
    assert_eq!(retries[0].matched, CiRetryMatch::Record);
    assert_eq!(retries[0].job_id, FAILED_RUN);
    assert_eq!(retries[0].workflow_run_id, Some(WORKFLOW));
    assert_eq!(flaky_retries(&named, &proved("Start the browser", 1)), None);
    assert_eq!(flaky_retries(&named, &proved("Seed the database", 2)), None);
    let generic = [job(failed(), Some("Run tests"), Some(FAILED_RUN))];
    assert_eq!(flaky_retries(&generic, &proved("Run tests", 2)), None);
    let not_actions = [job(failed(), Some("Start the browser"), None)];
    assert_eq!(
        flaky_retries(&not_actions, &proved("Start the browser", 2)),
        None
    );
    let twice = [
        job(failed(), Some("Start the browser"), Some(FAILED_RUN)),
        job(failed(), Some("Start the browser"), Some(RERUN)),
    ];
    assert_eq!(flaky_retries(&twice, &proved("Start the browser", 2)), None);
}

/// A quarantine matches its own failure only, by step and test, while a
/// known flake matches its job by name; a job that passed is no red.
#[test]
fn may_be_flaky_matches_quarantines_by_their_failure() {
    let quarantined = |step: &str| CiFlakes {
        known: Vec::new(),
        quarantined: vec![CiFailureIdentity {
            name: "test".to_string(),
            failed_step: Some(step.to_string()),
            failure: None,
        }],
        failures: Vec::new(),
    };
    let red = [job(failed(), Some("Run tests"), Some(FAILED_RUN))];

    assert!(may_be_flaky(&red, &quarantined("Run tests")));
    assert!(!may_be_flaky(&red, &quarantined("Build")));
    let known = CiFlakes {
        known: vec!["test".to_string()],
        ..CiFlakes::default()
    };
    assert!(may_be_flaky(&red, &known));
    let green = [job(CheckState::Passed, None, Some(FAILED_RUN))];
    assert!(!may_be_flaky(&green, &known));
}
