use crate::domain::standing_rules::RunRules;
use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::insights::{
    DurationSpread, EntryCount, EntryCountKind, EntrySummary, EntryTrouble, InsightsRecords,
    InsightsScope, RunInsightsRepository, RunSummary, StepSessionRef,
};
use crate::domain::models::{EntryId, NewRun, RunId, RunInsert, RunOutcome};
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
        .insert_run(NewRun {
            id,
            flow: "reply",
            request: "Say hi",
            user_id: &julia,
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
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

/// A Step whose agent asked the Run's owner a question, and the Step that
/// went on with the answer, are one visit of their entry: no revisit, no
/// trouble. The question, its Gate and the answer it took are left out of
/// the entry's outcomes, answers and Gates.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_question_and_the_step_going_on_from_it_are_one_visit(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = user("macro|julia@example.com");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request: "Plan the export",
            user_id: &julia,
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let key = |seq| crate::domain::models::StepKey { run_id: id, seq };
    let outcome = |name: &str| {
        crate::domain::models::Outcome::try_from(name.to_string()).expect("valid outcome")
    };
    for seq in [1, 2] {
        repo.start_step(key(seq), &entry("plan"), 1)
            .await
            .expect("start");
    }
    repo.end_step(key(1), &outcome("question"), "CSV or JSON?")
        .await
        .expect("end");
    repo.open_gate(key(1), "CSV or JSON?", &[outcome("answer")])
        .await
        .expect("open");
    repo.answer_gate(key(1), &outcome("answer"), Some("CSV"), &julia, None, None)
        .await
        .expect("answer");
    repo.end_step(key(2), &outcome("done"), "The plan")
        .await
        .expect("end");
    repo.open_gate(key(2), "Approve?", &[outcome("approve")])
        .await
        .expect("open");
    repo.answer_gate(key(2), &outcome("approve"), None, &julia, None, None)
        .await
        .expect("answer");

    let records = repo
        .read_insights(&InsightsScope {
            user_id: julia,
            from: at("2000-01-01T00:00:00Z"),
            to: at("2100-01-01T00:00:00Z"),
            flow: None,
            cap: 1000,
        })
        .await
        .expect("read");

    let plan = records
        .entries
        .iter()
        .find(|summary| summary.entry == entry("plan"))
        .expect("plan");
    assert_eq!(
        (
            plan.steps,
            plan.runs,
            plan.revisited_runs,
            plan.troubled_runs
        ),
        (2, 1, 0, 0)
    );
    assert_eq!((plan.gates_opened, plan.gates_answered), (1, 1));
    let counted: Vec<(EntryCountKind, &str)> = records
        .counts
        .iter()
        .map(|count| (count.kind, count.value.as_str()))
        .collect();
    assert_eq!(
        counted,
        [
            (EntryCountKind::Answer, "approve"),
            (EntryCountKind::Outcome, "done")
        ]
    );
    assert!(records.trouble.is_empty());
}

/// The Orchestrator's answers for the person are not the person's: their
/// Gates read as no Gate, as a rule's and the stand-in's do.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "insights"))
)]
async fn the_orchestrator_s_answers_are_left_out(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    sqlx::query!(
        r#"
        UPDATE factory_gate SET agent_session_id = $1
        WHERE answer IS NOT NULL AND rule IS NULL AND NOT by_stand_in
        "#,
        uuid::Uuid::now_v7(),
    )
    .execute(&pool)
    .await
    .expect("mark the answers the orchestrator's");

    let records = repo
        .read_insights(&november("macro|julia@example.com"))
        .await
        .expect("read");

    let plan = records
        .entries
        .iter()
        .find(|summary| summary.entry == entry("plan"))
        .expect("plan");
    assert_eq!(plan.gates_answered, 0);
    assert!(
        records
            .counts
            .iter()
            .all(|count| count.kind != EntryCountKind::Answer),
        "{:?}",
        records.counts
    );
}

/// A Step whose agent stopped without a result, and its owner retried, is
/// one visit with the Step that went on; one its owner then stopped fails
/// its entry, as its Flow routes it. Neither Gate, nor its answer, counts
/// among the entry's Gates and answers.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_step_without_a_result_is_one_visit_retried_and_a_failure_stopped(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = user("macro|julia@example.com");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request: "Plan the export",
            user_id: &julia,
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let key = |seq| crate::domain::models::StepKey { run_id: id, seq };
    let outcome = |name: &str| {
        crate::domain::models::Outcome::try_from(name.to_string()).expect("valid outcome")
    };
    for (seq, answer) in [(1, "retry"), (2, "stop")] {
        repo.start_step(key(seq), &entry("plan"), 1)
            .await
            .expect("start");
        repo.end_step(key(seq), &outcome("no_result"), "I stopped.")
            .await
            .expect("end");
        repo.open_gate(
            key(seq),
            "Retry or stop?",
            &[outcome("retry"), outcome("stop")],
        )
        .await
        .expect("open");
        repo.answer_gate(key(seq), &outcome(answer), None, &julia, None, None)
            .await
            .expect("answer");
    }

    let records = repo
        .read_insights(&InsightsScope {
            user_id: julia,
            from: at("2000-01-01T00:00:00Z"),
            to: at("2100-01-01T00:00:00Z"),
            flow: None,
            cap: 1000,
        })
        .await
        .expect("read");

    let plan = records
        .entries
        .iter()
        .find(|summary| summary.entry == entry("plan"))
        .expect("plan");
    assert_eq!(
        (
            plan.steps,
            plan.runs,
            plan.revisited_runs,
            plan.failed_runs,
            plan.troubled_runs
        ),
        (2, 1, 0, 1, 1)
    );
    assert_eq!((plan.gates_opened, plan.gates_answered), (0, 0));
    let counted: Vec<(EntryCountKind, &str, u32)> = records
        .counts
        .iter()
        .map(|count| (count.kind, count.value.as_str(), count.count))
        .collect();
    assert_eq!(counted, [(EntryCountKind::Outcome, "failed", 1)]);
    assert_eq!(
        records.trouble,
        [EntryTrouble {
            entry: entry("plan"),
            run_id: id,
            failed: true,
            visits: 1,
        }]
    );
}

/// A verifier's failed check after the merge (`fail`) and a failed deploy
/// (`deploy_failed`) are failures of their entries, as a Step that failed
/// for good is.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_failed_verify_or_deploy_is_a_failed_step(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = user("macro|julia@example.com");
    for (entry_id, ended) in [("verify", "fail"), ("deploy", "deploy_failed")] {
        let id = RunId::generate();
        let RunInsert::Inserted(_) = repo
            .insert_run(NewRun {
                id,
                flow: "code",
                request: "Ship it",
                user_id: &julia,
                task_id: None,
                repository: None,
                created_in_person: false,
                control_file_patterns: &[],
                control_files_team_id: None,
                standing_rules: RunRules::DEFAULT,
                automation: None,
            })
            .await
            .expect("insert")
        else {
            panic!("a run with no task is kept");
        };
        let key = crate::domain::models::StepKey { run_id: id, seq: 1 };
        repo.start_step(key, &entry(entry_id), 1)
            .await
            .expect("start");
        let outcome =
            crate::domain::models::Outcome::try_from(ended.to_string()).expect("valid outcome");
        repo.end_step(key, &outcome, "It failed")
            .await
            .expect("end");
    }

    let records = repo
        .read_insights(&InsightsScope {
            user_id: julia,
            from: at("2000-01-01T00:00:00Z"),
            to: at("2100-01-01T00:00:00Z"),
            flow: None,
            cap: 1000,
        })
        .await
        .expect("read");

    let mut trouble: Vec<(String, bool)> = records
        .trouble
        .iter()
        .map(|trouble| (trouble.entry.as_str().to_string(), trouble.failed))
        .collect();
    trouble.sort();
    assert_eq!(
        trouble,
        [("deploy".to_string(), true), ("verify".to_string(), true)]
    );
    let failed: Vec<(String, u32)> = records
        .entries
        .iter()
        .map(|entry| (entry.entry.as_str().to_string(), entry.failed_runs))
        .collect();
    assert!(failed.contains(&("verify".to_string(), 1)), "{failed:?}");
    assert!(failed.contains(&("deploy".to_string(), 1)), "{failed:?}");
}

/// A verifier that stopped without a result counts as the Flow routes its
/// owner's answer, beside the verifier's own failures: retried, it is one
/// visit with the Step that went on and failed (`fail`); stopped, it is a
/// `failed` Step. Both fail their entry, each Run once, and neither Gate
/// nor its answer counts.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_verifier_without_a_result_counts_with_the_verifier_s_failures(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = user("macro|julia@example.com");
    let outcome = |name: &str| {
        crate::domain::models::Outcome::try_from(name.to_string()).expect("valid outcome")
    };
    // Each Run's Steps of `verify`: what each ended with, and the owner's
    // answer to a Step without a result.
    let runs: [&[(&str, Option<&str>)]; 2] = [
        &[("no_result", Some("retry")), ("fail", None)],
        &[("no_result", Some("stop"))],
    ];
    let mut ids = Vec::new();
    for steps in runs {
        let id = RunId::generate();
        let RunInsert::Inserted(_) = repo
            .insert_run(NewRun {
                id,
                flow: "code",
                request: "Ship the toggle",
                user_id: &julia,
                task_id: None,
                repository: None,
                created_in_person: false,
                control_file_patterns: &[],
                control_files_team_id: None,
                standing_rules: RunRules::DEFAULT,
                automation: None,
            })
            .await
            .expect("insert")
        else {
            panic!("a run with no task is kept");
        };
        for (place, (ended, answer)) in steps.iter().enumerate() {
            let key = crate::domain::models::StepKey {
                run_id: id,
                seq: u16::try_from(place + 1).expect("few steps"),
            };
            repo.start_step(key, &entry("verify"), 1)
                .await
                .expect("start");
            repo.end_step(key, &outcome(ended), "The verifier's words")
                .await
                .expect("end");
            if let Some(answer) = answer {
                repo.open_gate(key, "Retry or stop?", &[outcome("retry"), outcome("stop")])
                    .await
                    .expect("open");
                repo.answer_gate(key, &outcome(answer), None, &julia, None, None)
                    .await
                    .expect("answer");
            }
        }
        ids.push(id);
    }

    let records = repo
        .read_insights(&InsightsScope {
            user_id: julia,
            from: at("2000-01-01T00:00:00Z"),
            to: at("2100-01-01T00:00:00Z"),
            flow: None,
            cap: 1000,
        })
        .await
        .expect("read");

    let verify = records
        .entries
        .iter()
        .find(|summary| summary.entry == entry("verify"))
        .expect("verify");
    assert_eq!(
        (
            verify.steps,
            verify.runs,
            verify.revisited_runs,
            verify.failed_runs,
            verify.troubled_runs
        ),
        (3, 2, 0, 2, 2)
    );
    assert_eq!((verify.gates_opened, verify.gates_answered), (0, 0));
    let counted: Vec<(EntryCountKind, &str, u32)> = records
        .counts
        .iter()
        .map(|count| (count.kind, count.value.as_str(), count.count))
        .collect();
    assert_eq!(
        counted,
        [
            (EntryCountKind::Outcome, "fail", 1),
            (EntryCountKind::Outcome, "failed", 1)
        ]
    );
    let mut trouble: Vec<(RunId, bool, u32)> = records
        .trouble
        .iter()
        .map(|trouble| (trouble.run_id, trouble.failed, trouble.visits))
        .collect();
    trouble.sort_by_key(|(id, ..)| ids.iter().position(|run| run == id));
    assert_eq!(trouble, [(ids[0], true, 1), (ids[1], true, 1)]);
}
