use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;

use super::PgRunRepository;
use crate::domain::models::{EntryId, Outcome, RunId, RunOutcome};
use crate::domain::ports::RunRepository;

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid entry id")
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_new_run_is_running_with_no_steps(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();

    let inserted = repo
        .insert_run(id, "Summarise the release notes")
        .await
        .expect("insert");
    let read = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run exists");

    assert_eq!(read, inserted);
    assert_eq!(read.request, "Summarise the release notes");
    assert_eq!(read.end, None);
    assert!(repo.list_steps(id).await.expect("steps").is_empty());
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn steps_record_their_visits_in_the_order_they_started(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Fix the flaky test")
        .await
        .expect("insert");

    repo.start_step(id, &entry("review"), 1)
        .await
        .expect("start");
    repo.end_step(
        id,
        &entry("review"),
        1,
        &outcome("blockers"),
        "Two blockers",
    )
    .await
    .expect("end");
    repo.start_step(id, &entry("fix"), 1).await.expect("start");
    repo.start_step(id, &entry("review"), 2)
        .await
        .expect("start");

    let steps = repo.list_steps(id).await.expect("steps");
    let visits: Vec<(&str, u16)> = steps
        .iter()
        .map(|step| (step.entry.as_str(), step.visit))
        .collect();
    assert_eq!(visits, [("review", 1), ("fix", 1), ("review", 2)]);

    let first = steps[0].end.as_ref().expect("the first step ended");
    assert_eq!(first.outcome, outcome("blockers"));
    assert_eq!(first.output, "Two blockers");
    assert_eq!(steps[1].end, None);
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_retried_step_start_keeps_one_step(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Reply to the request")
        .await
        .expect("insert");

    repo.start_step(id, &entry("reply"), 1)
        .await
        .expect("start");
    repo.start_step(id, &entry("reply"), 1)
        .await
        .expect("start again");

    assert_eq!(repo.list_steps(id).await.expect("steps").len(), 1);
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_run_that_ended_keeps_its_first_end(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Reply to the request")
        .await
        .expect("insert");

    repo.end_run(id, RunOutcome::Success).await.expect("end");
    repo.end_run(id, RunOutcome::Failed)
        .await
        .expect("end again");

    let run = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run exists");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Success));
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn deleting_a_run_removes_its_steps(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    repo.insert_run(id, "Reply to the request")
        .await
        .expect("insert");
    repo.start_step(id, &entry("reply"), 1)
        .await
        .expect("start");

    repo.delete_run(id).await.expect("delete");

    assert_eq!(repo.get_run(id).await.expect("get"), None);
    let steps: i64 = sqlx::query_scalar("SELECT count(*) FROM factory_step WHERE run_id = $1")
        .bind(id.as_uuid())
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(steps, 0);
}
