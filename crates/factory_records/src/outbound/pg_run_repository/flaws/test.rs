use crate::domain::standing_rules::RunRules;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use super::super::PgRunRepository;
use crate::domain::flaws::{FlawMarking, FlawRepository};
use crate::domain::models::{FlawMarker, NewRun, RunId, RunInsert, RunOutcome};
use crate::domain::ports::RunRepository;

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user")
}

/// A new Run of Julia's.
async fn run(repo: &PgRunRepository) -> RunId {
    let julia = julia();
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Add dark mode",
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
    id
}

/// A Run that ended `success` reads as flawed once marked, with who marked
/// it and why; a second mark keeps the first.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_succeeded_run_marked_flawed_keeps_its_first_mark(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = run(&repo).await;
    repo.end_run(id, RunOutcome::Success).await.expect("end");
    let person = FlawMarker::Person { user_id: julia() };

    let marked = repo
        .mark_flawed(id, &person, "The button is the wrong colour")
        .await
        .expect("mark");
    let again = repo
        .mark_flawed(
            id,
            &FlawMarker::Scorer {
                name: "visual-diff".to_string(),
            },
            "Off by a pixel",
        )
        .await
        .expect("mark again");

    let FlawMarking::Flawed(end) = marked else {
        panic!("the run was not marked: {marked:?}");
    };
    assert_eq!(end.outcome, RunOutcome::Flawed);
    let flaw = end.flaw.clone().expect("the end has its flaw");
    assert_eq!(flaw.by, person);
    assert_eq!(flaw.reason, "The button is the wrong colour");
    assert_eq!(again, FlawMarking::Flawed(end.clone()));
    let read = repo.get_run(id).await.expect("get").expect("exists");
    assert_eq!(read.end, Some(end));
}

/// A Scorer marks a Run by its name.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_scorer_marks_a_run_by_its_name(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = run(&repo).await;
    repo.end_run(id, RunOutcome::Success).await.expect("end");
    let scorer = FlawMarker::Scorer {
        name: "tests-pass".to_string(),
    };

    let marked = repo
        .mark_flawed(id, &scorer, "Two tests fail on main")
        .await
        .expect("mark");

    let FlawMarking::Flawed(end) = marked else {
        panic!("the run was not marked: {marked:?}");
    };
    assert_eq!(end.flaw.map(|flaw| flaw.by), Some(scorer));
}

/// Only a Run that ended `success` turns flawed: one still going, one that
/// ended another way, and one not recorded stay as they are.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn only_a_succeeded_run_turns_flawed(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let going = run(&repo).await;
    let broke = run(&repo).await;
    repo.end_run(broke, RunOutcome::Infrastructure)
        .await
        .expect("end");
    let person = FlawMarker::Person { user_id: julia() };

    let going_marked = repo
        .mark_flawed(going, &person, "Wrong")
        .await
        .expect("mark");
    let broke_marked = repo
        .mark_flawed(broke, &person, "Wrong")
        .await
        .expect("mark");
    let missing = repo
        .mark_flawed(RunId::generate(), &person, "Wrong")
        .await
        .expect("mark");

    assert_eq!(going_marked, FlawMarking::NotSucceeded(None));
    assert_eq!(
        broke_marked,
        FlawMarking::NotSucceeded(Some(RunOutcome::Infrastructure))
    );
    assert_eq!(missing, FlawMarking::NoSuchRun);
    let read = repo.get_run(broke).await.expect("get").expect("exists");
    assert_eq!(
        read.end.map(|end| (end.outcome, end.flaw)),
        Some((RunOutcome::Infrastructure, None))
    );
}

/// A person's mark names the Run's owner: the records refuse anyone else.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_persons_mark_names_the_runs_owner(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = run(&repo).await;
    repo.end_run(id, RunOutcome::Success).await.expect("end");
    let teo = MacroUserIdStr::try_from("macro|teo@example.com".to_string()).expect("user");

    let marked = repo
        .mark_flawed(id, &FlawMarker::Person { user_id: teo }, "Wrong")
        .await;

    assert!(marked.is_err(), "{marked:?}");
    let read = repo.get_run(id).await.expect("get").expect("exists");
    assert_eq!(read.end.map(|end| end.outcome), Some(RunOutcome::Success));
}
