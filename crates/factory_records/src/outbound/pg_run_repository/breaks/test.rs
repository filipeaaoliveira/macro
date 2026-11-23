use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use super::super::PgRunRepository;
use crate::domain::breaks::{BreakStatus, NewBreakRecord};
use crate::domain::models::{EntryId, NewRun, RunId, RunInsert, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;

const COMMIT: &str = "1111111111111111111111111111111111111111";

/// A new Run of Julia's with a started Step at each of `seqs`.
async fn run_with_steps(repo: &PgRunRepository, seqs: &[u16]) -> RunId {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Add dark mode",
            user_id: &julia,
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };
    let entry = EntryId::try_from("write".to_string()).expect("entry");
    for &seq in seqs {
        repo.start_step(StepKey { run_id: id, seq }, &entry, 1)
            .await
            .expect("start");
    }
    id
}

fn record(test: &str, status: BreakStatus, output: &str) -> NewBreakRecord {
    NewBreakRecord {
        test: test.to_string(),
        file: "crates/app/src/toggle.rs".to_string(),
        old_text: "saved = true".to_string(),
        new_text: "saved = false".to_string(),
        command: "cargo test -p app toggle_saves".to_string(),
        status,
        checked_commit: (status != BreakStatus::Unchecked).then(|| COMMIT.to_string()),
        output: output.to_string(),
    }
}

/// A Step keeps each test's first record, so a retried round changes
/// nothing; a Run's records read by Step, and other Runs' are apart.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_step_keeps_each_test_s_first_record_and_runs_read_apart(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run = run_with_steps(&repo, &[1, 3]).await;
    let other = run_with_steps(&repo, &[1]).await;
    let write = StepKey {
        run_id: run,
        seq: 1,
    };
    let fix = StepKey {
        run_id: run,
        seq: 3,
    };

    repo.record_break(
        fix,
        &record("toggle_keeps", BreakStatus::NotReproduced, "ok"),
    )
    .await
    .expect("record");
    repo.record_break(
        write,
        &record("toggle_saves", BreakStatus::Verified, "1 failed"),
    )
    .await
    .expect("record");
    // A retry's run of the same break.
    repo.record_break(write, &record("toggle_saves", BreakStatus::NotRun, "again"))
        .await
        .expect("record again");
    repo.record_break(
        StepKey {
            run_id: other,
            seq: 1,
        },
        &record("toggle_saves", BreakStatus::Unchecked, ""),
    )
    .await
    .expect("record");

    let records = repo
        .runs_break_records(&[run, other, RunId::generate()])
        .await
        .expect("read");

    assert_eq!(records.len(), 2);
    let kept: Vec<(u16, &str, BreakStatus, &str)> = records[&run]
        .iter()
        .map(|record| {
            (
                record.seq,
                record.test.as_str(),
                record.status,
                record.output.as_str(),
            )
        })
        .collect();
    assert_eq!(
        kept,
        vec![
            (1, "toggle_saves", BreakStatus::Verified, "1 failed"),
            (3, "toggle_keeps", BreakStatus::NotReproduced, "ok"),
        ]
    );
    assert_eq!(records[&run][0].checked_commit.as_deref(), Some(COMMIT));
    assert_eq!(records[&run][0].old_text, "saved = true");
    assert_eq!(records[&other][0].status, BreakStatus::Unchecked);
    assert_eq!(records[&other][0].checked_commit, None);
}

/// Every status this code writes is one the table takes and reads back.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn every_status_is_kept_and_read_back(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run = run_with_steps(&repo, &[1]).await;
    let key = StepKey {
        run_id: run,
        seq: 1,
    };
    let statuses = [
        BreakStatus::Verified,
        BreakStatus::NotReproduced,
        BreakStatus::NotRun,
        BreakStatus::NotApplied,
        BreakStatus::Unreadable,
        BreakStatus::Dropped,
        BreakStatus::Unchecked,
        BreakStatus::Interrupted,
        BreakStatus::Hung,
    ];
    for (at, status) in statuses.iter().enumerate() {
        repo.record_break(key, &record(&format!("test_{at}"), *status, ""))
            .await
            .expect("record");
    }

    let records = repo.runs_break_records(&[run]).await.expect("read");

    let read: Vec<BreakStatus> = records[&run].iter().map(|record| record.status).collect();
    assert_eq!(read, statuses);
}

/// An interrupted run's record is replaced by the next run's; that one,
/// as any other, stays.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn an_interrupted_record_is_replaced_by_the_next_run_s(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run = run_with_steps(&repo, &[1]).await;
    let key = StepKey {
        run_id: run,
        seq: 1,
    };

    repo.record_break(
        key,
        &record("toggle_saves", BreakStatus::Interrupted, "cut"),
    )
    .await
    .expect("record");
    repo.record_break(
        key,
        &record("toggle_saves", BreakStatus::Verified, "1 failed"),
    )
    .await
    .expect("record again");
    repo.record_break(key, &record("toggle_saves", BreakStatus::NotRun, "later"))
        .await
        .expect("record a third time");

    let records = repo.runs_break_records(&[run]).await.expect("read");
    assert_eq!(records[&run].len(), 1);
    assert_eq!(records[&run][0].status, BreakStatus::Verified);
    assert_eq!(records[&run][0].output, "1 failed");
}
