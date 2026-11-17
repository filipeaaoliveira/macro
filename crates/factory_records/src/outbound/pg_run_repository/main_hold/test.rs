use chrono::{TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::main_hold::MainHoldRepository;
use crate::domain::models::{Repository, RunId, RunInsert};
use crate::domain::ports::RunRepository;

const RED_HEAD: &str = "2222222222222222222222222222222222222222";
const TASK: &str = "00000000-0000-7000-8000-000000000101";

fn main_of_app() -> Repository {
    Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    }
}

/// A new `code` Run of Julia's on `repository`.
async fn code_run(repo: &PgRunRepository, repository: Option<&Repository>) -> RunId {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(id, "code", "Add dark mode", &julia, None, repository)
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };
    id
}

/// A base is held from the first red look on: a later red look moves its
/// head and failed checks, never its start; a green look clears it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_red_base_is_held_until_a_green_look_clears_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();
    assert_eq!(repo.main_hold(&base).await.expect("read"), None);

    let first = repo
        .hold_main(&base, RED_HEAD, "- test (failure)")
        .await
        .expect("hold");
    let again = repo
        .hold_main(
            &base,
            "3333333333333333333333333333333333333333",
            "- lint (failure)",
        )
        .await
        .expect("hold again");
    assert_eq!(again.since, first.since);
    assert_eq!(again.head_sha, "3333333333333333333333333333333333333333");
    assert_eq!(again.failed, "- lint (failure)");
    assert_eq!(repo.main_hold(&base).await.expect("read"), Some(again));

    repo.clear_main_hold(&base, Utc::now())
        .await
        .expect("clear");
    assert_eq!(repo.main_hold(&base).await.expect("read"), None);

    // Another branch of the repository was never held.
    let release = Repository {
        url: base.url.clone(),
        base_branch: "release".to_string(),
    };
    assert_eq!(repo.main_hold(&release).await.expect("read"), None);
}

/// One red period claims its task once: a second claim fails, a released
/// claim can be made again, and a cleared hold starts a new period with a
/// claim of its own.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "tasks"))
)]
async fn a_red_period_claims_its_task_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();
    let hold = repo
        .hold_main(&base, RED_HEAD, "- test (failure)")
        .await
        .expect("hold");
    assert!(!hold.task_claimed);

    assert!(
        repo.claim_hold_task(&base, hold.since)
            .await
            .expect("claim")
    );
    assert!(
        !repo
            .claim_hold_task(&base, hold.since)
            .await
            .expect("claim again")
    );
    repo.release_hold_task(&base, hold.since)
        .await
        .expect("release");
    assert!(
        repo.claim_hold_task(&base, hold.since)
            .await
            .expect("claim after release")
    );

    let task = Uuid::parse_str(TASK).expect("uuid");
    repo.record_hold_task(&base, hold.since, task)
        .await
        .expect("record task");
    // A release after the task was filed keeps the claim.
    repo.release_hold_task(&base, hold.since)
        .await
        .expect("release");
    let held = repo.main_hold(&base).await.expect("read").expect("held");
    assert_eq!(held.task_id, Some(task));
    assert!(held.task_claimed);
    assert!(
        !repo
            .claim_hold_task(&base, hold.since)
            .await
            .expect("claim filed")
    );

    // A claim for another period's start is not this one's.
    let earlier = hold.since - TimeDelta::seconds(1);
    assert!(
        !repo
            .claim_hold_task(&base, earlier)
            .await
            .expect("claim stale")
    );

    repo.clear_main_hold(&base, Utc::now())
        .await
        .expect("clear");
    let next = repo
        .hold_main(&base, RED_HEAD, "- test (failure)")
        .await
        .expect("hold");
    assert_eq!(next.task_id, None);
    assert!(
        repo.claim_hold_task(&base, next.since)
            .await
            .expect("claim next")
    );
}

/// The suspects are the base's merges since it was last green, oldest
/// first, newest kept at the limit; a merge recorded twice counts once, and a
/// Run on no repository records none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn the_suspects_are_the_merges_since_the_base_was_last_green(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();

    let before = code_run(&repo, Some(&base)).await;
    repo.record_merge(before, 1, "https://github.com/macro/app/pull/1", Utc::now())
        .await
        .expect("merge");
    repo.clear_main_hold(&base, Utc::now())
        .await
        .expect("green");

    let mut after = Vec::new();
    for number in 2..=4 {
        let run = code_run(&repo, Some(&base)).await;
        let url = format!("https://github.com/macro/app/pull/{number}");
        let merged_at = Utc::now();
        repo.record_merge(run, number, &url, merged_at)
            .await
            .expect("merge");
        repo.record_merge(run, number, &url, merged_at)
            .await
            .expect("merge again");
        after.push(run);
    }
    let elsewhere = Repository {
        url: "https://github.com/macro/other".to_string(),
        base_branch: "main".to_string(),
    };
    let other = code_run(&repo, Some(&elsewhere)).await;
    repo.record_merge(
        other,
        9,
        "https://github.com/macro/other/pull/9",
        Utc::now(),
    )
    .await
    .expect("merge elsewhere");
    let no_code = code_run(&repo, None).await;
    repo.record_merge(
        no_code,
        5,
        "https://github.com/macro/app/pull/5",
        Utc::now(),
    )
    .await
    .expect("no repository");

    let suspects = repo.suspect_merges(&base, 10).await.expect("suspects");
    let numbers: Vec<u64> = suspects.iter().map(|merge| merge.number).collect();
    assert_eq!(numbers, vec![2, 3, 4]);
    assert_eq!(suspects[0].run_id, after[0]);
    assert_eq!(suspects[0].url, "https://github.com/macro/app/pull/2");

    let newest = repo.suspect_merges(&base, 2).await.expect("suspects");
    let numbers: Vec<u64> = newest.iter().map(|merge| merge.number).collect();
    assert_eq!(numbers, vec![3, 4]);
}
