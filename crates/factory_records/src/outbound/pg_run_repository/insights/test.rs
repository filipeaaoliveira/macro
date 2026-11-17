use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::insights::{
    DurationSpread, EntryCount, EntryCountKind, EntrySummary, EntryTrouble, InsightsRecords,
    InsightsScope, RunInsightsRepository, RunSummary, StepSessionRef,
};
use crate::domain::models::{EntryId, RunId, RunInsert, RunOutcome};
use crate::domain::ports::RunRepository;
use crate::outbound::pg_run_repository::PgRunRepository;

const EXPORT: &str = "00000000-0000-7000-8000-000000000201";
const IMPORT: &str = "00000000-0000-7000-8000-000000000202";
const THREAD: &str = "00000000-0000-7000-8000-000000000203";
const DARK_MODE: &str = "00000000-0000-7000-8000-000000000204";
const HELLO: &str = "00000000-0000-7000-8000-000000000205";
const BILLING: &str = "00000000-0000-7000-8000-000000000206";

fn run(id: &str) -> RunId {
    RunId::from_uuid(Uuid::parse_str(id).expect("valid run id"))
}

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid entry id")
}

fn at(time: &str) -> DateTime<Utc> {
    time.parse().expect("valid time")
}

fn user(id: &str) -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from(id.to_string()).expect("valid user id")
}

/// Julia's Runs created in November 2026, of every Flow.
fn november(user_id: &str) -> InsightsScope {
    InsightsScope {
        user_id: user(user_id),
        from: at("2026-11-01T00:00:00Z"),
        to: at("2026-12-01T00:00:00Z"),
        flow: None,
        cap: 1000,
    }
}

fn ids(records: &InsightsRecords) -> Vec<RunId> {
    records.runs.iter().map(|run| run.id).collect()
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights"))
)]
async fn the_persons_runs_in_the_range_are_read_newest_first(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo
        .read_insights(&november("macro|julia@example.com"))
        .await
        .expect("read");

    // Teo's Run and Julia's October Run are not hers in November.
    assert_eq!(
        ids(&records),
        vec![
            run(HELLO),
            run(DARK_MODE),
            run(THREAD),
            run(IMPORT),
            run(EXPORT)
        ]
    );
    assert_eq!(
        records.runs.last(),
        Some(&RunSummary {
            id: run(EXPORT),
            title: "Plan the export".to_string(),
            flow: Some("plan".to_string()),
            created_at: at("2026-11-10T09:00:00Z"),
            end: Some((RunOutcome::Success, at("2026-11-10T09:30:00Z"))),
        })
    );
    assert_eq!(records.runs[0].flow, None);
    assert_eq!(records.runs[2].end, None);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights"))
)]
async fn another_persons_runs_are_never_read(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo
        .read_insights(&november("macro|teo@example.com"))
        .await
        .expect("read");

    assert_eq!(ids(&records), vec![run(BILLING)]);
    assert_eq!(records.entries.len(), 1);
    assert_eq!(records.entries[0].steps, 1);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights"))
)]
async fn a_flow_a_range_and_the_cap_narrow_the_runs(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = november("macro|julia@example.com");

    let plans = InsightsScope {
        flow: Some("plan".to_string()),
        ..julia.clone()
    };
    let one_day = InsightsScope {
        from: at("2026-11-11T00:00:00Z"),
        to: at("2026-11-12T00:00:00Z"),
        ..julia.clone()
    };
    let newest_two = InsightsScope { cap: 2, ..julia };

    let plans = repo.read_insights(&plans).await.expect("plans");
    let one_day = repo.read_insights(&one_day).await.expect("one day");
    let newest_two = repo.read_insights(&newest_two).await.expect("newest two");

    assert_eq!(ids(&plans), vec![run(DARK_MODE), run(IMPORT), run(EXPORT)]);
    assert_eq!(ids(&one_day), vec![run(IMPORT)]);
    assert_eq!(ids(&newest_two), vec![run(HELLO), run(DARK_MODE)]);
    // The Steps read are the capped Runs' only.
    assert_eq!(
        newest_two
            .entries
            .iter()
            .map(|summary| (summary.entry.as_str(), summary.steps))
            .collect::<Vec<_>>(),
        vec![("plan", 1), ("write", 1)]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights"))
)]
async fn each_entry_sums_up_its_steps_gates_and_loops(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo
        .read_insights(&november("macro|julia@example.com"))
        .await
        .expect("read");

    assert_eq!(
        records.entries,
        vec![
            // Four plan Steps: twice in the export (sent back once), the
            // failed import, the dark mode. They took 300, 180, 600 and
            // 300 s; the three Gates waited 600, 120 and 60 s.
            EntrySummary {
                entry: entry("plan"),
                steps: 4,
                runs: 3,
                revisited_runs: 1,
                failed_runs: 1,
                troubled_runs: 2,
                duration: Some(DurationSpread {
                    count: 4,
                    median_seconds: 300.0,
                    p90_seconds: 510.0,
                }),
                gates_opened: 3,
                gates_answered: 3,
                gate_wait: Some(DurationSpread {
                    count: 3,
                    median_seconds: 120.0,
                    p90_seconds: 504.0,
                }),
            },
            // The thread's reply still goes: no time yet.
            EntrySummary {
                entry: entry("reply"),
                steps: 1,
                runs: 1,
                revisited_runs: 0,
                failed_runs: 0,
                troubled_runs: 0,
                duration: None,
                gates_opened: 0,
                gates_answered: 0,
                gate_wait: None,
            },
            // The dark mode's write was cancelled, so only the export's
            // 600 s count.
            EntrySummary {
                entry: entry("write"),
                steps: 2,
                runs: 2,
                revisited_runs: 0,
                failed_runs: 0,
                troubled_runs: 0,
                duration: Some(DurationSpread {
                    count: 1,
                    median_seconds: 600.0,
                    p90_seconds: 600.0,
                }),
                gates_opened: 0,
                gates_answered: 0,
                gate_wait: None,
            },
        ]
    );

    let count = |entry_id: &str, kind, value: &str, count| EntryCount {
        entry: entry(entry_id),
        kind,
        value: value.to_string(),
        count,
    };
    assert_eq!(
        records.counts,
        vec![
            count("plan", EntryCountKind::Answer, "approve", 2),
            count("plan", EntryCountKind::Answer, "revise", 1),
            count("plan", EntryCountKind::Outcome, "done", 3),
            count("plan", EntryCountKind::Outcome, "failed", 1),
            count("write", EntryCountKind::Outcome, "cancelled", 1),
            count("write", EntryCountKind::Outcome, "done", 1),
        ]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights"))
)]
async fn the_runs_where_an_entry_failed_or_looped_are_named_newest_first(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo
        .read_insights(&november("macro|julia@example.com"))
        .await
        .expect("read");

    assert_eq!(
        records.trouble,
        vec![
            EntryTrouble {
                entry: entry("plan"),
                run_id: run(IMPORT),
                failed: true,
                visits: 1,
            },
            EntryTrouble {
                entry: entry("plan"),
                run_id: run(EXPORT),
                failed: false,
                visits: 2,
            },
        ]
    );
    assert_eq!(
        records.sessions,
        vec![StepSessionRef {
            run_id: run(EXPORT),
            entry: entry("write"),
            session_id: Uuid::parse_str("00000000-0000-7000-8000-000000000301")
                .expect("valid session id"),
        }]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_new_run_is_read_with_its_flow(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    let julia = user("macro|julia@example.com");
    let RunInsert::Inserted(inserted) = repo
        .insert_run(id, "reply", "Say hi", &julia, None, None, false)
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };

    let records = repo
        .read_insights(&InsightsScope {
            user_id: julia,
            from: inserted.created_at,
            to: inserted.created_at + chrono::Duration::seconds(1),
            flow: Some("reply".to_string()),
            cap: 10,
        })
        .await
        .expect("read");

    assert_eq!(ids(&records), vec![id]);
    assert_eq!(records.runs[0].flow, Some("reply".to_string()));
    assert_eq!(records.entries, Vec::new());
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_person_with_no_runs_reads_nothing(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo
        .read_insights(&november("macro|julia@example.com"))
        .await
        .expect("read");

    assert_eq!(records, InsightsRecords::default());
}

fn plan(n: u32) -> RunId {
    run(&format!("00000000-0000-7000-8000-0000000005{n:02}"))
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights_failed_plans"))
)]
async fn an_entry_names_at_most_its_five_newest_troubled_runs(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo
        .read_insights(&november("macro|julia@example.com"))
        .await
        .expect("read");

    // All seven failed at plan, and the newest also went back to it: it
    // counts once among the troubled Runs. The five newest are named.
    assert_eq!(records.entries[0].failed_runs, 7);
    assert_eq!(records.entries[0].revisited_runs, 1);
    assert_eq!(records.entries[0].troubled_runs, 7);
    assert_eq!(
        records
            .trouble
            .iter()
            .map(|trouble| trouble.run_id)
            .collect::<Vec<_>>(),
        vec![plan(7), plan(6), plan(5), plan(4), plan(3)]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights_failed_plans"))
)]
async fn a_period_with_more_runs_than_the_cap_says_so(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = november("macro|julia@example.com");

    let exactly = repo
        .read_insights(&InsightsScope {
            cap: 7,
            ..julia.clone()
        })
        .await
        .expect("read at the cap");
    let over = repo
        .read_insights(&InsightsScope { cap: 6, ..julia })
        .await
        .expect("read past the cap");

    assert!(!exactly.capped);
    assert_eq!(exactly.runs.len(), 7);
    assert!(over.capped);
    // The oldest is left out, and so are its Steps.
    assert_eq!(over.runs.len(), 6);
    assert_eq!(over.runs.last().map(|run| run.id), Some(plan(2)));
    assert_eq!(over.entries[0].steps, 7);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights"))
)]
async fn a_step_a_cancelled_run_stopped_takes_no_time(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let records = repo
        .read_insights(&november("macro|julia@example.com"))
        .await
        .expect("read");

    // Two write Steps: the export's took 600 s; the dark mode's was
    // cancelled after 840 s and is not timed.
    let write = records
        .entries
        .iter()
        .find(|summary| summary.entry == entry("write"))
        .expect("write");
    assert_eq!(write.steps, 2);
    assert_eq!(
        write.duration,
        Some(DurationSpread {
            count: 1,
            median_seconds: 600.0,
            p90_seconds: 600.0,
        })
    );
}
