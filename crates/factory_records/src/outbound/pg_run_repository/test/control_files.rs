//! A Run's Tampered files, the approval a Gate's answer gives them, and a
//! person's Control File patterns.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;

use super::{entry, insert, julia, key, outcome};
use crate::domain::models::{RunId, Tampering};
use crate::domain::ports::RunRepository;
use crate::outbound::pg_run_repository::PgRunRepository;

/// The Tampered files read last replace the ones before, an unlisted
/// branch keeps that it was, and none clears it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_keeps_the_tampered_files_read_last(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    let run = insert(&repo, id, "Fix the login bug").await;
    assert_eq!(run.tampering, None);

    let tampered = Tampering {
        files: vec![
            ".github/workflows/ci.yml".to_string(),
            "src/login.test.ts".to_string(),
        ],
        unlisted: false,
    };
    repo.set_tampering(id, Some(&tampered)).await.expect("set");
    let read = repo.get_run(id).await.expect("get").expect("kept");
    assert_eq!(read.tampering, Some(tampered));

    let unlisted = Tampering {
        files: Vec::new(),
        unlisted: true,
    };
    repo.set_tampering(id, Some(&unlisted)).await.expect("set");
    let read = repo.get_run(id).await.expect("get").expect("kept");
    assert_eq!(read.tampering, Some(unlisted));

    repo.set_tampering(id, None).await.expect("clear");
    let read = repo.get_run(id).await.expect("get").expect("kept");
    assert_eq!(read.tampering, None);
}

/// An answer keeps the Tampered files it approved; a later answer to the
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
    let approved = vec![".github/workflows/ci.yml".to_string()];

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

/// A person with no patterns of their own has none kept; theirs replace
/// the ones before, and clearing them goes back to none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_person_keeps_their_own_control_file_patterns(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    assert_eq!(repo.control_files(&julia()).await.expect("read"), None);

    let first = vec!["ci/**".to_string()];
    let second = vec!["**/*.test.ts".to_string(), "Makefile".to_string()];
    repo.set_control_files(&julia(), Some(&first))
        .await
        .expect("set");
    repo.set_control_files(&julia(), Some(&second))
        .await
        .expect("replace");
    assert_eq!(
        repo.control_files(&julia()).await.expect("read"),
        Some(second)
    );

    repo.set_control_files(&julia(), Some(&[]))
        .await
        .expect("empty");
    assert_eq!(
        repo.control_files(&julia()).await.expect("read"),
        Some(Vec::new())
    );

    repo.set_control_files(&julia(), None).await.expect("clear");
    assert_eq!(repo.control_files(&julia()).await.expect("read"), None);
}
