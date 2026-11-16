use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use super::PgRunRepository;
use crate::domain::models::{EntryId, Outcome, RunId, RunOutcome, TaskLink};
use crate::domain::ports::RunRepository;

fn entry(id: &str) -> EntryId {
    EntryId::try_from(id.to_string()).expect("valid entry id")
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_new_run_is_running_with_no_steps(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();

    let inserted = repo
        .insert_run(id, "Summarise the release notes", &julia())
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

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn steps_record_their_visits_in_the_order_they_started(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Fix the flaky test", &julia())
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

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_retried_step_start_keeps_one_step(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Reply to the request", &julia())
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

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_run_that_ended_keeps_its_first_end(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Reply to the request", &julia())
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

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn deleting_a_run_removes_its_steps(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    repo.insert_run(id, "Reply to the request", &julia())
        .await
        .expect("insert");
    repo.start_step(id, &entry("reply"), 1)
        .await
        .expect("start");

    repo.delete_run(id).await.expect("delete");

    assert_eq!(repo.get_run(id).await.expect("get"), None);
    let steps = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM factory_step WHERE run_id = $1"#,
        id.as_uuid()
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(steps, 0);
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_gate_waits_then_keeps_its_first_answer(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Plan the migration", &julia())
        .await
        .expect("insert");
    repo.start_step(id, &entry("plan"), 1).await.expect("start");
    let answers = [outcome("approve"), outcome("revise")];

    repo.open_gate(id, &entry("plan"), 1, "Approve the plan?", &answers)
        .await
        .expect("open");
    repo.open_gate(id, &entry("plan"), 1, "Approve the plan?", &answers)
        .await
        .expect("open again");
    let waiting = repo.list_steps(id).await.expect("steps");
    let gate = waiting[0].gate.as_ref().expect("the step has a gate");
    assert_eq!(gate.question, "Approve the plan?");
    assert_eq!(gate.answers, answers);
    assert_eq!(gate.answer, None);

    repo.answer_gate(
        id,
        &entry("plan"),
        1,
        &outcome("revise"),
        Some("Keep the old index"),
        &julia(),
    )
    .await
    .expect("answer");
    repo.answer_gate(id, &entry("plan"), 1, &outcome("approve"), None, &julia())
        .await
        .expect("answer again");

    let answered = repo.list_steps(id).await.expect("steps");
    let answer = answered[0]
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref())
        .expect("the gate is answered");
    assert_eq!(answer.answer, outcome("revise"));
    assert_eq!(answer.note.as_deref(), Some("Keep the old index"));
    assert_eq!(answer.user_id, Some(julia()));
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn an_answer_from_an_unknown_user_records_no_one(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Plan the migration", &julia())
        .await
        .expect("insert");
    repo.start_step(id, &entry("plan"), 1).await.expect("start");
    repo.open_gate(
        id,
        &entry("plan"),
        1,
        "Approve the plan?",
        &[outcome("approve")],
    )
    .await
    .expect("open");

    // The API refuses an unknown user, but an account can go between that
    // check and the workflow recording the answer.
    let gone = MacroUserIdStr::try_from("macro|gone@example.com".to_string()).expect("valid");
    repo.answer_gate(id, &entry("plan"), 1, &outcome("approve"), None, &gone)
        .await
        .expect("answer");

    let steps = repo.list_steps(id).await.expect("steps");
    let answer = steps[0]
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref())
        .expect("the gate is answered");
    assert_eq!(answer.answer, outcome("approve"));
    assert_eq!(answer.user_id, None);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_run_can_end_cancelled(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Plan the migration", &julia())
        .await
        .expect("insert");

    repo.end_run(id, RunOutcome::Cancelled).await.expect("end");

    let run = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run exists");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Cancelled));
}

const LOGIN_TASK: &str = "00000000-0000-7000-8000-000000000101";
const DARK_MODE_TASK: &str = "00000000-0000-7000-8000-000000000102";

fn task(id: &str) -> uuid::Uuid {
    uuid::Uuid::parse_str(id).expect("valid task id")
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "tasks"))
)]
async fn a_run_links_to_one_task_at_a_time(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Fix the login bug", &julia())
        .await
        .expect("insert");

    let first = repo.link_task(id, task(LOGIN_TASK)).await.expect("link");
    let second = repo
        .link_task(id, task(DARK_MODE_TASK))
        .await
        .expect("relink");

    assert_eq!((first, second), (TaskLink::Linked, TaskLink::Linked));
    let run = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run exists");
    assert_eq!(run.task_id, Some(task(DARK_MODE_TASK)));
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people"))
)]
async fn a_task_that_does_not_exist_or_a_run_that_does_not_is_not_linked(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    repo.insert_run(id, "Fix the login bug", &julia())
        .await
        .expect("insert");

    let no_task = repo.link_task(id, task(LOGIN_TASK)).await.expect("link");
    let no_run = repo
        .link_task(RunId::generate(), task(LOGIN_TASK))
        .await
        .expect("link");

    assert_eq!(no_task, TaskLink::NoSuchTask);
    assert_eq!(no_run, TaskLink::NoSuchRun);
    let run = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run exists");
    assert_eq!(run.task_id, None);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("people", "tasks"))
)]
async fn deleting_its_task_unlinks_a_run_and_keeps_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    repo.insert_run(id, "Fix the login bug", &julia())
        .await
        .expect("insert");
    repo.link_task(id, task(LOGIN_TASK)).await.expect("link");

    sqlx::query!(r#"DELETE FROM "Document" WHERE id = $1"#, LOGIN_TASK)
        .execute(&pool)
        .await
        .expect("delete the task");

    let run = repo
        .get_run(id)
        .await
        .expect("get")
        .expect("the run is kept");
    assert_eq!(run.task_id, None);
}
