//! The Runs waiting on their owner: those that have not ended and wait on
//! a Gate no one answered yet.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;

use super::{entry, insert, julia, key, outcome, teo};
use crate::domain::models::{NewRun, RunId, RunInsert, RunOutcome};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;
use crate::outbound::pg_run_repository::PgRunRepository;

/// A Run waits on its owner from when its Gate opens until it is
/// answered or the Run ends; Runs with no Gate, and other people's, are
/// left out, and the newest come first, up to the limit.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_waits_on_its_owner_while_its_gate_is_open(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let answers = [outcome("approve"), outcome("revise")];

    let older = RunId::generate();
    insert(&repo, older, "Plan the migration").await;
    let newer = RunId::generate();
    insert(&repo, newer, "Plan the rollback").await;
    let working = RunId::generate();
    insert(&repo, working, "Plan the backfill").await;
    for id in [older, newer, working] {
        repo.start_step(key(id, 1), &entry("plan"), 1)
            .await
            .expect("start");
    }
    for id in [older, newer] {
        repo.open_gate(key(id, 1), "Approve the plan?", &answers)
            .await
            .expect("open");
    }
    // Someone else's Run waiting on its Gate is theirs to list.
    let theirs = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: theirs,
            flow: "plan",
            request: "Plan the cutover",
            user_id: &teo(),
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
    repo.start_step(key(theirs, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.open_gate(key(theirs, 1), "Approve the plan?", &answers)
        .await
        .expect("open");

    let ids = |runs: Vec<crate::domain::models::Run>| -> Vec<RunId> {
        runs.into_iter().map(|run| run.id).collect()
    };
    let waiting = repo.list_waiting_runs(&julia(), 10).await.expect("list");
    assert_eq!(ids(waiting), [newer, older]);
    let newest = repo.list_waiting_runs(&julia(), 1).await.expect("list");
    assert_eq!(ids(newest), [newer]);

    repo.answer_gate(key(older, 1), &outcome("approve"), None, &julia(), None)
        .await
        .expect("answer");
    repo.end_run(newer, RunOutcome::Cancelled)
        .await
        .expect("cancel");
    let waiting = repo.list_waiting_runs(&julia(), 10).await.expect("list");
    assert!(waiting.is_empty());
}
