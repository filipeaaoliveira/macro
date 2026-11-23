use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::insights::InsightsScope;
use crate::domain::models::{RunId, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::quality::{
    CiRetryMatch, CiRetryRecord, CiRetryRequest, CiRetryResult, FlakyCiFailure, NewCiQuarantine,
    QualityScope, RunQualityRepository,
};
use crate::outbound::pg_run_repository::PgRunRepository;

const DASHES: &str = "00000000-0000-7000-8000-000000000401";
const PARSER: &str = "00000000-0000-7000-8000-000000000403";
const APP: &str = "https://github.com/macro/app";
const HEAD: &str = "4444444444444444444444444444444444444444";

fn run(id: &str) -> RunId {
    RunId::from_uuid(Uuid::parse_str(id).expect("valid run id"))
}

fn at(time: &str) -> DateTime<Utc> {
    time.parse().expect("valid time")
}

fn retry(name: &str, job_id: u64, failed_step: Option<&str>) -> CiRetryRequest {
    CiRetryRequest {
        name: name.to_string(),
        job_id,
        failed_step: failed_step.map(str::to_string),
        failure: None,
        matched: CiRetryMatch::Record,
    }
}

/// The failures other pull requests of the repository had count toward a
/// Run's flaky red, never its own, each once per pull request; the reruns
/// other Runs asked for count too, since a passed rerun keeps its red apart
/// from its green wait.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_run_reads_the_failures_of_other_pull_requests(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let since = at("2026-11-01T00:00:00Z");

    let flakes = repo
        .ci_flakes(run(PARSER), APP, since)
        .await
        .expect("flakes");

    assert_eq!(
        flakes.failures,
        vec![
            FlakyCiFailure {
                name: "test".to_string(),
                failed_step: Some("Run tests".to_string()),
                failure: None,
                pull_requests: 2,
            },
            FlakyCiFailure {
                name: "build".to_string(),
                failed_step: Some("Set up job".to_string()),
                failure: None,
                pull_requests: 1,
            },
        ]
    );
    assert!(flakes.known.is_empty());
    assert!(flakes.quarantined.is_empty());

    // A rerun the parser Run asked for counts for the dashes Run, on the
    // parser's pull request.
    repo.request_ci_retries(
        StepKey {
            run_id: run(PARSER),
            seq: 1,
        },
        HEAD,
        &[retry("lint", 9, Some("Lint the docs"))],
    )
    .await
    .expect("request");
    let flakes = repo
        .ci_flakes(run(DASHES), APP, since)
        .await
        .expect("flakes");
    assert!(flakes.failures.contains(&FlakyCiFailure {
        name: "lint".to_string(),
        failed_step: Some("Lint the docs".to_string()),
        failure: None,
        pull_requests: 1,
    }));
    // Failures before the period are not read.
    let later = repo
        .ci_flakes(run(PARSER), APP, at("2026-12-01T00:00:00Z"))
        .await
        .expect("flakes");
    assert!(later.failures.is_empty());
}

/// A Step reruns a job once: asking again keeps the first rerun, and a
/// rerun that ended keeps how it ended.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_step_reruns_a_job_once_and_keeps_how_it_ended(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let key = StepKey {
        run_id: run(PARSER),
        seq: 1,
    };

    repo.request_ci_retries(key, HEAD, &[retry("test", 7, Some("Run tests"))])
        .await
        .expect("request");
    repo.request_ci_retries(key, HEAD, &[retry("test", 8, None)])
        .await
        .expect("request again");
    let open = repo.ci_retries(key).await.expect("retries");
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].job_id, 7);
    assert_eq!(open[0].head_sha, HEAD);
    assert_eq!(open[0].matched, CiRetryMatch::Record);
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

/// A job whose reruns passed often enough is quarantined once in its
/// repository, for the Run that crossed the threshold to file its task;
/// the Steps on the repository then read it as quarantined, and Insights
/// lists it with the person's reruns.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_job_whose_reruns_passed_is_quarantined_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let since = at("2026-11-01T00:00:00Z");
    let key = StepKey {
        run_id: run(PARSER),
        seq: 1,
    };
    repo.request_ci_retries(key, HEAD, &[retry("test", 7, Some("Run tests"))])
        .await
        .expect("request");
    let job = |task: &str| NewCiQuarantine {
        name: "test".to_string(),
        task_id: Uuid::parse_str(task).expect("valid task id"),
    };
    let task = "00000000-0000-7000-8000-000000000c01";

    // An open rerun is no passed retry.
    let none = repo
        .quarantine_ci_jobs(run(PARSER), APP, &[job(task)], since, 1)
        .await
        .expect("quarantine");
    assert!(none.is_empty());
    repo.settle_ci_retries(key, &[("test".to_string(), CiRetryResult::Passed)])
        .await
        .expect("settle");
    // Short of the threshold, nothing is quarantined.
    let short = repo
        .quarantine_ci_jobs(run(PARSER), APP, &[job(task)], since, 2)
        .await
        .expect("quarantine");
    assert!(short.is_empty());
    let made = repo
        .quarantine_ci_jobs(run(PARSER), APP, &[job(task)], since, 1)
        .await
        .expect("quarantine");
    assert_eq!(made, vec!["test".to_string()]);
    let again = repo
        .quarantine_ci_jobs(
            run(DASHES),
            APP,
            &[job("00000000-0000-7000-8000-000000000c02")],
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
    assert_eq!(unfiled[0].passed_retries, 1);
    assert_eq!(unfiled[0].task_id.to_string(), task);
    repo.mark_ci_quarantine_filed(APP, "test")
        .await
        .expect("filed");
    assert!(
        repo.unfiled_ci_quarantines(run(PARSER))
            .await
            .expect("unfiled")
            .is_empty()
    );
    let flakes = repo
        .ci_flakes(run(DASHES), APP, since)
        .await
        .expect("flakes");
    assert_eq!(flakes.quarantined, vec!["test".to_string()]);

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
        .ci_flakes(run(PARSER), APP, at("2026-11-01T00:00:00Z"))
        .await
        .expect("flakes");
    assert_eq!(flakes.known, names);
}
