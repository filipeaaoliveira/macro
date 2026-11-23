use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::insights::InsightsScope;
use crate::domain::models::{EntryId, RunId, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::quality::{
    CiFailureIdentity, CiJob, CiJobResult, CiRetryMatch, CiRetryRecord, CiRetryRequest,
    CiRetryResult, CiVerdict, CiWait, FlakyCiFailure, NewCiQuarantine, QualityScope,
    RunQualityRepository,
};
use crate::outbound::pg_run_repository::PgRunRepository;

const DASHES: &str = "00000000-0000-7000-8000-000000000401";
const PARSER: &str = "00000000-0000-7000-8000-000000000403";
const TEO: &str = "00000000-0000-7000-8000-000000000405";
const APP: &str = "https://github.com/macro/app";
const PARSER_HEAD: &str = "4444444444444444444444444444444444444444";
const TEO_HEAD: &str = "5555555555555555555555555555555555555555";

fn run(id: &str) -> RunId {
    RunId::from_uuid(Uuid::parse_str(id).expect("valid run id"))
}

fn at(time: &str) -> DateTime<Utc> {
    time.parse().expect("valid time")
}

fn step(id: &str, seq: u16) -> StepKey {
    StepKey {
        run_id: run(id),
        seq,
    }
}

fn retry(name: &str, job_id: u64, failed_step: Option<&str>) -> CiRetryRequest {
    CiRetryRequest {
        name: name.to_string(),
        job_id,
        workflow_run_id: Some(900),
        failed_step: failed_step.map(str::to_string),
        failure: None,
        matched: CiRetryMatch::Record,
    }
}

fn identity(name: &str, failed_step: Option<&str>) -> CiFailureIdentity {
    CiFailureIdentity {
        name: name.to_string(),
        failed_step: failed_step.map(str::to_string),
        failure: None,
    }
}

/// A failure proves flaky on another pull request when its rerun passed,
/// or when the same Run saw the same head green after it; a red nothing
/// proved flaky counts for nothing. Each pull request counts once, and
/// neither the asking Run nor its own pull request counts.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn only_failures_proved_flaky_on_other_pull_requests_count(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let since = at("2026-11-01T00:00:00Z");

    // The fixture's reds were all fixed by new commits: none is proved.
    let unproved = repo
        .ci_flakes(run(PARSER), APP, None, since)
        .await
        .expect("flakes");
    assert!(unproved.failures.is_empty());

    // Teo's head goes green after its red, with no new commit.
    let teo_green = step(TEO, 2);
    repo.start_step(teo_green, &EntryId::try_from("ci".to_string()).unwrap(), 2)
        .await
        .expect("step");
    repo.record_ci_wait(&CiWait {
        key: teo_green,
        head_sha: TEO_HEAD.to_string(),
        verdict: CiVerdict::Green,
        jobs: vec![CiJob {
            name: "test".to_string(),
            result: CiJobResult::Passed,
            failed_step: None,
            failure: None,
            replaced: false,
        }],
    })
    .await
    .expect("green");
    // The dashes Run's rerun of the same failure passed.
    let dashes_ci = step(DASHES, 6);
    repo.request_ci_retries(
        dashes_ci,
        PARSER_HEAD,
        &[retry("test", 7, Some("Run tests"))],
    )
    .await
    .expect("request");
    repo.settle_ci_retries(dashes_ci, &[("test".to_string(), CiRetryResult::Passed)])
        .await
        .expect("settle");

    let proved = repo
        .ci_flakes(run(PARSER), APP, None, since)
        .await
        .expect("flakes");
    assert_eq!(
        proved.failures,
        vec![FlakyCiFailure {
            name: "test".to_string(),
            failed_step: Some("Run tests".to_string()),
            failure: None,
            pull_requests: 2,
        }]
    );
    // The asking pull request's own proof does not count.
    let own_pull_request = repo
        .ci_flakes(
            run(PARSER),
            APP,
            Some("https://github.com/macro/app/pull/1"),
            since,
        )
        .await
        .expect("flakes");
    assert_eq!(own_pull_request.failures[0].pull_requests, 1);
    // Nor do Runs older than the period.
    let later = repo
        .ci_flakes(run(PARSER), APP, None, at("2026-12-01T00:00:00Z"))
        .await
        .expect("flakes");
    assert!(later.failures.is_empty());
}

/// A Step reruns a job once: asking again keeps the first rerun and gives
/// back no name, so only the first attempt asks GitHub; a rerun that ended
/// keeps how it ended.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_step_reruns_a_job_once_and_keeps_how_it_ended(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let key = step(PARSER, 1);

    let first = repo
        .request_ci_retries(key, PARSER_HEAD, &[retry("test", 7, Some("Run tests"))])
        .await
        .expect("request");
    let again = repo
        .request_ci_retries(key, PARSER_HEAD, &[retry("test", 8, None)])
        .await
        .expect("request again");
    assert_eq!(first, vec!["test".to_string()]);
    assert!(again.is_empty());
    let open = repo.ci_retries(key).await.expect("retries");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].job_id, 7);
    assert_eq!(open[0].workflow_run_id, Some(900));
    assert_eq!(open[0].failed_step.as_deref(), Some("Run tests"));
    assert_eq!(open[0].result, None);

    repo.settle_ci_retries(key, &[("test".to_string(), CiRetryResult::Passed)])
        .await
        .expect("settle");
    repo.settle_ci_retries(key, &[("test".to_string(), CiRetryResult::Failed)])
        .await
        .expect("settle again");
    let settled = repo.ci_retries(key).await.expect("retries");
    assert_eq!(settled[0].result, Some(CiRetryResult::Passed));
}

/// Two attempts of a Step asking at once: exactly one keeps the rerun,
/// and only it may ask GitHub.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn of_two_attempts_asking_at_once_one_keeps_the_rerun(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let key = step(PARSER, 1);
    let asked = [retry("test", 7, Some("Run tests"))];

    let (one, other) = futures::join!(
        repo.request_ci_retries(key, PARSER_HEAD, &asked),
        repo.request_ci_retries(key, PARSER_HEAD, &asked),
    );

    let mut kept = one.expect("one");
    kept.extend(other.expect("other"));
    assert_eq!(kept, vec!["test".to_string()]);
}

/// A failure whose reruns passed often enough is quarantined once in its
/// repository, by its job, step and test; passing again keeps it and its
/// task. It lapses once no rerun passed in the period, and a lift removes
/// it. Insights lists it with the person's reruns.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_failure_whose_reruns_passed_is_quarantined_until_lifted(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let since = at("2026-11-01T00:00:00Z");
    let key = step(PARSER, 1);
    repo.request_ci_retries(key, PARSER_HEAD, &[retry("test", 7, Some("Run tests"))])
        .await
        .expect("request");
    let job = |failed_step: Option<&str>, task: &str| NewCiQuarantine {
        failure: identity("test", failed_step),
        task_id: Uuid::parse_str(task).expect("valid task id"),
    };
    let task = "00000000-0000-7000-8000-000000000c01";

    // An open rerun is no passed retry.
    let none = repo
        .quarantine_ci_jobs(run(PARSER), APP, &[job(Some("Run tests"), task)], since, 1)
        .await
        .expect("quarantine");
    assert!(none.is_empty());
    repo.settle_ci_retries(key, &[("test".to_string(), CiRetryResult::Passed)])
        .await
        .expect("settle");
    // Short of the threshold, or another failure of the job, nothing.
    let short = repo
        .quarantine_ci_jobs(run(PARSER), APP, &[job(Some("Run tests"), task)], since, 2)
        .await
        .expect("quarantine");
    assert!(short.is_empty());
    let other_step = repo
        .quarantine_ci_jobs(run(PARSER), APP, &[job(Some("Lint"), task)], since, 1)
        .await
        .expect("quarantine");
    assert!(other_step.is_empty());
    let made = repo
        .quarantine_ci_jobs(run(PARSER), APP, &[job(Some("Run tests"), task)], since, 1)
        .await
        .expect("quarantine");
    assert_eq!(made, vec!["test".to_string()]);
    let again = repo
        .quarantine_ci_jobs(
            run(DASHES),
            APP,
            &[job(
                Some("Run tests"),
                "00000000-0000-7000-8000-000000000c02",
            )],
            since,
            1,
        )
        .await
        .expect("quarantine again");
    assert!(again.is_empty());

    let unfiled = repo
        .unfiled_ci_quarantines(run(PARSER))
        .await
        .expect("unfiled");
    assert_eq!(unfiled.len(), 1);
    assert_eq!(unfiled[0].repository, APP);
    assert_eq!(unfiled[0].failure, identity("test", Some("Run tests")));
    assert_eq!(unfiled[0].task_id.to_string(), task);
    let task_id = unfiled[0].task_id;
    repo.mark_ci_quarantine_filed(task_id).await.expect("filed");
    assert!(
        repo.unfiled_ci_quarantines(run(PARSER))
            .await
            .expect("unfiled")
            .is_empty()
    );
    let flakes = repo
        .ci_flakes(run(DASHES), APP, None, since)
        .await
        .expect("flakes");
    assert_eq!(
        flakes.quarantined,
        vec![identity("test", Some("Run tests"))]
    );
    let lapsed = repo
        .ci_flakes(
            run(DASHES),
            APP,
            None,
            Utc::now() + chrono::TimeDelta::days(1),
        )
        .await
        .expect("flakes");
    assert!(lapsed.quarantined.is_empty());

    let records = repo
        .read_quality(&QualityScope {
            runs: InsightsScope {
                user_id: MacroUserIdStr::try_from("macro|julia@example.com".to_string())
                    .expect("valid user id"),
                from: since,
                to: at("2026-12-01T00:00:00Z"),
                flow: None,
                cap: 1000,
            },
            repository: None,
        })
        .await
        .expect("read");
    assert_eq!(
        records.ci_retries,
        vec![CiRetryRecord {
            run_id: run(PARSER),
            name: "test".to_string(),
            result: Some(CiRetryResult::Passed),
        }]
    );
    assert_eq!(records.ci_quarantines.len(), 1);
    assert!(records.ci_quarantines[0].filed);

    assert!(repo.lift_ci_quarantine(task_id).await.expect("lift"));
    assert!(!repo.lift_ci_quarantine(task_id).await.expect("lift again"));
    let lifted = repo
        .ci_flakes(run(DASHES), APP, None, since)
        .await
        .expect("flakes");
    assert!(lifted.quarantined.is_empty());
}

/// The known flakes a Run keeps are read back for its Steps.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_run_keeps_its_known_flakes(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let names = vec!["e2e".to_string(), "macos".to_string()];

    repo.keep_known_ci_flakes(run(PARSER), &names)
        .await
        .expect("keep");
    repo.keep_known_ci_flakes(run(PARSER), &names)
        .await
        .expect("keep again");

    let flakes = repo
        .ci_flakes(run(PARSER), APP, None, at("2026-11-01T00:00:00Z"))
        .await
        .expect("flakes");
    assert_eq!(flakes.known, names);
}
