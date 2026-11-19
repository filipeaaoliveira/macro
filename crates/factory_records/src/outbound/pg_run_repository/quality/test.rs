use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::insights::InsightsScope;
use crate::domain::models::{RunId, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::quality::{
    CiJob, CiJobResult, CiVerdict, CiWait, LastCiWait, QualityRecords, QualityScope,
    QualitySession, RunQualityRepository,
};
use crate::domain::review_rounds::ReviewRoundOutcome;
use crate::outbound::pg_run_repository::PgRunRepository;

const DASHES: &str = "00000000-0000-7000-8000-000000000401";
const SPACES: &str = "00000000-0000-7000-8000-000000000402";
const PARSER: &str = "00000000-0000-7000-8000-000000000403";
const FOOTER: &str = "00000000-0000-7000-8000-000000000404";

const APP: &str = "https://github.com/macro/app";
const SITE: &str = "https://github.com/macro/site";

fn run(id: &str) -> RunId {
    RunId::from_uuid(Uuid::parse_str(id).expect("valid run id"))
}

fn at(time: &str) -> DateTime<Utc> {
    time.parse().expect("valid time")
}

fn november(user_id: &str, repository: Option<&str>) -> QualityScope {
    QualityScope {
        runs: InsightsScope {
            user_id: MacroUserIdStr::try_from(user_id.to_string()).expect("valid user id"),
            from: at("2026-11-01T00:00:00Z"),
            to: at("2026-12-01T00:00:00Z"),
            flow: None,
            cap: 1000,
        },
        repository: repository.map(str::to_string),
    }
}

fn julia(repository: Option<&str>) -> QualityScope {
    november("macro|julia@example.com", repository)
}

fn ids(records: &QualityRecords) -> Vec<RunId> {
    records.runs.iter().map(|run| run.run.id).collect()
}

fn job(name: &str, result: CiJobResult, failed_step: Option<&str>) -> CiJob {
    CiJob {
        name: name.to_string(),
        result,
        failed_step: failed_step.map(str::to_string),
        failure: None,
        replaced: false,
    }
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn the_persons_runs_say_whether_they_merged_and_whether_a_person_stepped_in(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo.read_quality(&julia(None)).await.expect("read");

    // Teo's Run is not Julia's.
    assert_eq!(
        ids(&records),
        vec![run(FOOTER), run(PARSER), run(SPACES), run(DASHES)]
    );
    let flags: Vec<_> = records
        .runs
        .iter()
        .map(|run| (run.merged, run.person_answered, run.steered))
        .collect();
    assert_eq!(
        flags,
        vec![
            // Her Orchestrator answered: not a person.
            (true, false, false),
            (false, false, true),
            (true, true, false),
            // A Standing Rule answered: not a person.
            (true, false, false),
        ]
    );
    assert_eq!(records.runs[3].repository.as_deref(), Some(APP));
    assert_eq!(
        records.runs[3].pull_request.as_deref(),
        Some("https://github.com/macro/app/pull/1")
    );
    // Each session once, with the bot that ran it; the scope reviewer's
    // Step is from before Steps recorded their bot.
    let session = |id: &str, bot: Option<&str>| QualitySession {
        id: Uuid::parse_str(id).unwrap(),
        bot: bot.map(|bot| Uuid::parse_str(bot).unwrap()),
    };
    let bot = Some("00000000-0000-7000-8000-000000000a01");
    assert_eq!(
        records.runs[3].sessions,
        vec![
            session("00000000-0000-7000-8000-000000000501", bot),
            session("00000000-0000-7000-8000-000000000502", bot),
            session("00000000-0000-7000-8000-000000000503", None),
            session("00000000-0000-7000-8000-000000000504", bot),
        ]
    );
    assert_eq!(
        records.repositories,
        vec![APP.to_string(), SITE.to_string()]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn ci_waits_review_rounds_and_blockers_are_read_with_their_runs(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo.read_quality(&julia(None)).await.expect("read");

    let waits: Vec<_> = records
        .ci_waits
        .iter()
        .map(|wait| (wait.run_id, wait.seq, wait.verdict, wait.jobs.len()))
        .collect();
    // Teo's red wait is not read.
    assert_eq!(waits.len(), 4);
    assert!(waits.contains(&(run(DASHES), 6, CiVerdict::Red, 2)));
    assert!(waits.contains(&(run(DASHES), 7, CiVerdict::Green, 2)));
    assert!(waits.contains(&(run(SPACES), 1, CiVerdict::Green, 2)));
    let first = records
        .ci_waits
        .iter()
        .find(|wait| wait.run_id == run(DASHES) && wait.seq == 6)
        .expect("red wait");
    assert_eq!(first.started_at, at("2026-11-10T10:00:00Z"));
    assert_eq!(first.ended_at, at("2026-11-10T10:20:00Z"));
    assert_eq!(
        first.jobs,
        vec![
            job("build", CiJobResult::Setup, Some("Set up job")),
            job("test", CiJobResult::Failed, Some("Run tests")),
        ]
    );

    assert_eq!(
        records
            .review_rounds
            .iter()
            .map(|round| (round.seq, round.outcome))
            .collect::<Vec<_>>(),
        vec![
            (2, Some(ReviewRoundOutcome::Changes)),
            (5, Some(ReviewRoundOutcome::Changes))
        ]
    );
    assert_eq!(
        records
            .reviewers
            .iter()
            .map(|reviewer| (
                reviewer.seq,
                reviewer.round_seq,
                reviewer.role.as_deref(),
                reviewer.agent.as_deref()
            ))
            .collect::<Vec<_>>(),
        vec![
            (2, 2, Some("correctness-reviewer"), Some("primary")),
            (3, 2, Some("scope-reviewer"), Some("backup")),
            (5, 5, Some("correctness-reviewer"), Some("primary")),
        ]
    );
    // A follow-up is no Blocker.
    assert_eq!(
        records
            .blockers
            .iter()
            .map(|blocker| (blocker.seq, blocker.round_seq, blocker.problem.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (2, 2, "The empty string panics"),
            (5, 5, "The empty string panics"),
        ]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_repository_narrows_the_runs_but_not_the_repositories_to_pick(pool: PgPool) {
    // A Run keeps its repository as it was written, in any case.
    sqlx::query!(
        "UPDATE factory_run SET repo_url = 'https://github.com/Macro/Site' WHERE repo_url = $1",
        SITE,
    )
    .execute(&pool)
    .await
    .expect("rename");
    // Two Runs on one repository, written in two cases, list it once.
    sqlx::query!(
        "UPDATE factory_run SET repo_url = 'https://github.com/Macro/App' WHERE id = $1",
        run(SPACES).as_uuid(),
    )
    .execute(&pool)
    .await
    .expect("rename");
    let repo = PgRunRepository::new(pool);

    let site = repo.read_quality(&julia(Some(SITE))).await.expect("site");
    let none = repo
        .read_quality(&julia(Some("https://github.com/macro/none")))
        .await
        .expect("none");

    assert_eq!(ids(&site), vec![run(FOOTER)]);
    assert!(site.ci_waits.is_empty());
    assert_eq!(
        site.repositories
            .iter()
            .map(|named| named.to_lowercase())
            .collect::<Vec<_>>(),
        vec![APP.to_string(), SITE.to_string()]
    );
    assert!(
        site.repositories
            .contains(&"https://github.com/Macro/Site".to_string())
    );
    assert!(none.runs.is_empty());
    assert_eq!(none.repositories.len(), 2);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn a_ci_wait_is_kept_with_its_jobs_and_a_retry_keeps_its_final_verdict(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let key = StepKey {
        run_id: run(SPACES),
        seq: 2,
    };
    let wait = CiWait {
        key,
        head_sha: "6666666666666666666666666666666666666666".to_string(),
        verdict: CiVerdict::Red,
        jobs: vec![
            job("lint", CiJobResult::Passed, None),
            CiJob {
                failure: Some("src/parser.rs: parser::test::empty".to_string()),
                ..job("test", CiJobResult::Failed, Some("Run tests"))
            },
            // The same name, cancelled on the head a newer push replaced.
            CiJob {
                replaced: true,
                ..job("test", CiJobResult::Cancelled, None)
            },
        ],
    };
    let kept = |records: &QualityRecords| {
        records
            .ci_waits
            .iter()
            .find(|kept| kept.run_id == key.run_id && kept.seq == key.seq)
            .cloned()
            .expect("kept")
    };

    repo.record_ci_wait(&wait).await.expect("record");
    let first = kept(&repo.read_quality(&julia(None)).await.expect("read"));
    // The same head and verdict again keeps the wait as it was.
    let again = CiWait {
        jobs: vec![job("test", CiJobResult::Failed, None)],
        ..wait.clone()
    };
    repo.record_ci_wait(&again).await.expect("record again");
    let unchanged = kept(&repo.read_quality(&julia(None)).await.expect("read"));
    // A retry that saw the failed job pass on the same head replaces it.
    let green = CiWait {
        verdict: CiVerdict::Green,
        jobs: vec![job("test", CiJobResult::Passed, None)],
        ..wait.clone()
    };
    repo.record_ci_wait(&green).await.expect("record green");
    repo.record_ci_wait(&green)
        .await
        .expect("record green again");
    let last = kept(&repo.read_quality(&julia(None)).await.expect("read"));

    assert_eq!(first.verdict, CiVerdict::Red);
    assert_eq!(
        first.jobs,
        vec![
            wait.jobs[0].clone(),
            wait.jobs[1].clone(),
            wait.jobs[2].clone()
        ]
    );
    assert_eq!(unchanged, first);
    assert_eq!(last.verdict, CiVerdict::Green);
    assert_eq!(last.jobs, green.jobs);
    assert!(last.ended_at >= first.ended_at);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "quality"))
)]
async fn the_last_ci_wait_is_the_latest_before_a_step_with_its_heads_jobs(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    // A job of a head the wait replaced is not one of its head's.
    repo.record_ci_wait(&CiWait {
        key: StepKey {
            run_id: run(SPACES),
            seq: 2,
        },
        head_sha: "6666666666666666666666666666666666666666".to_string(),
        verdict: CiVerdict::Red,
        jobs: vec![
            job("test", CiJobResult::Failed, None),
            CiJob {
                replaced: true,
                ..job("e2e", CiJobResult::Cancelled, None)
            },
        ],
    })
    .await
    .expect("record");

    let before_second = repo.last_ci_wait(run(DASHES), 7).await.expect("read");
    let after_both = repo.last_ci_wait(run(DASHES), 9).await.expect("read");
    let before_any = repo.last_ci_wait(run(DASHES), 6).await.expect("read");
    let spaces = repo.last_ci_wait(run(SPACES), 5).await.expect("read");

    assert_eq!(
        before_second,
        Some(LastCiWait {
            head_sha: "1111111111111111111111111111111111111111".to_string(),
            jobs: vec!["build".to_string(), "test".to_string()],
        })
    );
    assert_eq!(
        after_both.map(|wait| wait.head_sha).as_deref(),
        Some("2222222222222222222222222222222222222222")
    );
    assert_eq!(before_any, None);
    assert_eq!(spaces.map(|wait| wait.jobs), Some(vec!["test".to_string()]));
}
