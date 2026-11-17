//! A Run's Control File changes, the approval a Gate's answer gives them,
//! and a team's Control File patterns.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::{entry, insert, julia, key, outcome};
use crate::domain::models::{ControlFileChanges, RunId, RunInsert, TeamMembership};
use crate::domain::ports::RunRepository;
use crate::outbound::pg_run_repository::PgRunRepository;

fn files(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

/// A Run keeps whether it was created in person.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_keeps_whether_it_was_created_in_person(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    for in_person in [true, false] {
        let id = RunId::generate();
        let RunInsert::Inserted(inserted) = repo
            .insert_run(id, "plan", "Fix it", &julia(), None, None, in_person)
            .await
            .expect("insert")
        else {
            panic!("a run with no task is kept");
        };
        let read = repo.get_run(id).await.expect("get").expect("kept");

        assert_eq!(inserted.created_in_person, in_person);
        assert_eq!(read.created_in_person, in_person);
    }
}

/// The Control File changes read last replace the ones before, unlisted
/// changes keep that they were, and none clears them.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_keeps_the_control_files_read_last(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    let run = insert(&repo, id, "Fix the login bug").await;
    assert_eq!(run.control_files, None);

    let listed = ControlFileChanges {
        tampered: files(&[".github/workflows/ci.yml", "src/login.test.ts"]),
        named: files(&["Makefile"]),
        unlisted: false,
    };
    repo.set_control_file_changes(id, Some(&listed))
        .await
        .expect("set");
    let read = repo.get_run(id).await.expect("get").expect("kept");
    assert_eq!(read.control_files, Some(listed));

    let unlisted = ControlFileChanges {
        tampered: Vec::new(),
        named: Vec::new(),
        unlisted: true,
    };
    repo.set_control_file_changes(id, Some(&unlisted))
        .await
        .expect("set");
    let read = repo.get_run(id).await.expect("get").expect("kept");
    assert_eq!(read.control_files, Some(unlisted));

    repo.set_control_file_changes(id, None)
        .await
        .expect("clear");
    let read = repo.get_run(id).await.expect("get").expect("kept");
    assert_eq!(read.control_files, None);
}

/// An answer keeps the Control Files it approved; a later answer to the
/// same Gate changes nothing.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn an_answer_keeps_the_control_files_it_approved(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Fix the login bug").await;
    for seq in [1, 2] {
        repo.start_step(key(id, seq), &entry("merge-gate"), seq)
            .await
            .expect("start");
        repo.open_gate(key(id, seq), "Merge?", &[outcome("approve")])
            .await
            .expect("open");
    }
    let approved = files(&[".github/workflows/ci.yml"]);

    repo.answer_gate(
        key(id, 1),
        &outcome("approve"),
        None,
        &julia(),
        Some(&approved),
    )
    .await
    .expect("answer");
    repo.answer_gate(key(id, 1), &outcome("approve"), None, &julia(), None)
        .await
        .expect("answer again");
    repo.answer_gate(key(id, 2), &outcome("approve"), None, &julia(), None)
        .await
        .expect("answer");

    let steps = repo.list_steps(id).await.expect("steps");
    let approvals: Vec<Option<Vec<String>>> = steps
        .iter()
        .map(|step| {
            step.gate
                .as_ref()
                .and_then(|gate| gate.answer.as_ref())
                .expect("answered")
                .approved_control_files
                .clone()
        })
        .collect();
    assert_eq!(approvals, [Some(approved), None]);
}

/// A person's team and whether they administer it read from its members;
/// someone on no team has none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_person_s_team_reads_with_whether_they_administer_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let team_id = Uuid::parse_str("00000000-0000-7000-8000-000000000201").expect("valid");
    let teo = macro_user_id::user_id::MacroUserIdStr::try_from("macro|teo@example.com".to_string())
        .expect("valid");
    let nobody =
        macro_user_id::user_id::MacroUserIdStr::try_from("macro|nobody@example.com".to_string())
            .expect("valid");

    assert_eq!(
        repo.team_of(&julia()).await.expect("read"),
        Some(TeamMembership {
            team_id,
            admin: true,
        })
    );
    assert_eq!(
        repo.team_of(&teo).await.expect("read"),
        Some(TeamMembership {
            team_id,
            admin: false,
        })
    );
    assert_eq!(repo.team_of(&nobody).await.expect("read"), None);
}

/// A team with no patterns of its own has none kept; its own replace the
/// ones before, and clearing them goes back to none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_team_keeps_its_own_control_file_patterns(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let team_id = Uuid::parse_str("00000000-0000-7000-8000-000000000201").expect("valid");
    assert_eq!(repo.team_control_files(team_id).await.expect("read"), None);

    let first = files(&["ci/**"]);
    let second = files(&["**/*.test.ts", "Makefile"]);
    repo.set_team_control_files(team_id, Some(&first))
        .await
        .expect("set");
    repo.set_team_control_files(team_id, Some(&second))
        .await
        .expect("replace");
    assert_eq!(
        repo.team_control_files(team_id).await.expect("read"),
        Some(second)
    );

    repo.set_team_control_files(team_id, Some(&[]))
        .await
        .expect("empty");
    assert_eq!(
        repo.team_control_files(team_id).await.expect("read"),
        Some(Vec::new())
    );

    repo.set_team_control_files(team_id, None)
        .await
        .expect("clear");
    assert_eq!(repo.team_control_files(team_id).await.expect("read"), None);
}
