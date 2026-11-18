use crate::domain::standing_rules::RunRules;
use chrono::{DateTime, TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::main_hold::{MainHoldRepository, RequiredChecksState};
use crate::domain::models::{NewRun, Repository, RunId, RunInsert};
use crate::domain::ports::RunRepository;

const RED_HEAD: &str = "2222222222222222222222222222222222222222";
const TASK: &str = "00000000-0000-7000-8000-000000000101";

fn a_minute_ago() -> DateTime<Utc> {
    Utc::now() - TimeDelta::minutes(1)
}

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
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Add dark mode",
            user_id: &julia,
            task_id: None,
            repository,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
        })
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
        .hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    let again = repo
        .hold_main(
            &base,
            "3333333333333333333333333333333333333333",
            "- lint (failure)",
            Utc::now(),
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
        .hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    assert!(!hold.task_claimed);

    assert!(
        repo.claim_hold_task(&base, hold.since, Utc::now())
            .await
            .expect("claim")
            .is_some()
    );
    assert!(
        repo.claim_hold_task(&base, hold.since, Utc::now())
            .await
            .expect("claim again")
            .is_none()
    );
    repo.release_hold_task(&base, hold.since)
        .await
        .expect("release");
    assert!(
        repo.claim_hold_task(&base, hold.since, Utc::now())
            .await
            .expect("claim after release")
            .is_some()
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
        repo.claim_hold_task(&base, hold.since, Utc::now())
            .await
            .expect("claim filed")
            .is_none()
    );

    // A claim for another period's start is not this one's.
    let earlier = hold.since - TimeDelta::seconds(1);
    assert!(
        repo.claim_hold_task(&base, earlier, Utc::now())
            .await
            .expect("claim stale")
            .is_none()
    );

    repo.clear_main_hold(&base, Utc::now())
        .await
        .expect("clear");
    let next = repo
        .hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    assert_eq!(next.task_id, None);
    assert!(
        repo.claim_hold_task(&base, next.since, Utc::now())
            .await
            .expect("claim next")
            .is_some()
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

/// A green read that started before the hold's red one clears nothing; one
/// that started after it clears the hold.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_green_read_older_than_the_hold_clears_nothing(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();
    let red_at = a_minute_ago();
    repo.hold_main(&base, RED_HEAD, "- test (failure)", red_at)
        .await
        .expect("hold");

    repo.clear_main_hold(&base, red_at - TimeDelta::seconds(1))
        .await
        .expect("stale green");
    assert!(repo.main_hold(&base).await.expect("read").is_some());

    repo.clear_main_hold(&base, Utc::now())
        .await
        .expect("green");
    assert_eq!(repo.main_hold(&base).await.expect("read"), None);
}

/// A claim that never got its task is overtaken once it is stale, so a
/// look that died between the claim and the filing does not skip the red
/// period; a fresh claim is not.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_stale_claim_with_no_task_is_claimed_again(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();
    let hold = repo
        .hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    let now = Utc::now();
    assert!(
        repo.claim_hold_task(&base, hold.since, now)
            .await
            .expect("claim")
            .is_some()
    );
    let soon = now + TimeDelta::minutes(14);
    assert!(
        repo.claim_hold_task(&base, hold.since, soon)
            .await
            .expect("fresh")
            .is_none()
    );
    let later = now + TimeDelta::minutes(16);
    assert!(
        repo.claim_hold_task(&base, hold.since, later)
            .await
            .expect("stale")
            .is_some()
    );
}

/// A task filed and deleted since is not filed again in the same red
/// period, however old its claim.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "tasks"))
)]
async fn a_deleted_task_is_not_filed_again(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let base = main_of_app();
    let hold = repo
        .hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    let now = Utc::now();
    assert!(
        repo.claim_hold_task(&base, hold.since, now)
            .await
            .expect("claim")
            .is_some()
    );
    let task = Uuid::parse_str(TASK).expect("uuid");
    repo.record_hold_task(&base, hold.since, task)
        .await
        .expect("record");

    sqlx::query!(r#"DELETE FROM "Document" WHERE id = $1"#, TASK)
        .execute(&pool)
        .await
        .expect("delete the task");
    let held = repo.main_hold(&base).await.expect("read").expect("held");
    assert_eq!(held.task_id, None);
    let much_later = now + TimeDelta::hours(1);
    assert!(
        repo.claim_hold_task(&base, hold.since, much_later)
            .await
            .expect("claim again")
            .is_none()
    );
}

/// Required checks that could not be read leave the base unheld and say
/// so; a later red or green read forgets it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn unread_required_checks_hold_nothing(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();
    repo.hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");

    repo.mark_required(&base, RequiredChecksState::Unread)
        .await
        .expect("unread");
    let state = repo.base_state(&base).await.expect("read");
    assert_eq!(state.hold, None);
    assert_eq!(state.required, Some(RequiredChecksState::Unread));

    repo.hold_main(&base, RED_HEAD, "- test (failure)", Utc::now())
        .await
        .expect("hold");
    let state = repo.base_state(&base).await.expect("read");
    assert!(state.hold.is_some());
    assert_eq!(state.required, None);

    repo.mark_required(&base, RequiredChecksState::None)
        .await
        .expect("none");
    let state = repo.base_state(&base).await.expect("read");
    assert_eq!(state.required, Some(RequiredChecksState::None));
}

/// A base's recent merges are its newest first, none before any.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_base_s_recent_merges_are_newest_first(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();
    assert!(repo.recent_merges(&base, 5).await.expect("read").is_empty());

    let older = code_run(&repo, Some(&base)).await;
    let newer = code_run(&repo, Some(&base)).await;
    repo.record_merge(newer, 2, "https://github.com/macro/app/pull/2", Utc::now())
        .await
        .expect("merge");
    repo.record_merge(
        older,
        1,
        "https://github.com/macro/app/pull/1",
        a_minute_ago(),
    )
    .await
    .expect("merge");
    let recent = repo.recent_merges(&base, 5).await.expect("read");
    let runs: Vec<RunId> = recent.iter().map(|merge| merge.run_id).collect();
    assert_eq!(runs, vec![newer, older]);
    assert_eq!(repo.recent_merges(&base, 1).await.expect("read").len(), 1);
}

/// A red period's task is filed under one id: the first claim chooses it,
/// and a claim after a release or a stale claim keeps it, so a filing whose
/// answer was lost is asked again under it. The next red period chooses its
/// own.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_red_period_files_its_task_under_one_id(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let base = main_of_app();
    let hold = repo
        .hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");

    let now = Utc::now();
    let first = repo
        .claim_hold_task(&base, hold.since, now)
        .await
        .expect("claim")
        .expect("claimed");
    repo.release_hold_task(&base, hold.since)
        .await
        .expect("release");
    let again = repo
        .claim_hold_task(&base, hold.since, now)
        .await
        .expect("claim after release")
        .expect("claimed");
    let stale = repo
        .claim_hold_task(&base, hold.since, now + TimeDelta::minutes(16))
        .await
        .expect("claim stale")
        .expect("claimed");
    assert_eq!(again, first);
    assert_eq!(stale, first);

    repo.clear_main_hold(&base, Utc::now())
        .await
        .expect("clear");
    let next = repo
        .hold_main(&base, RED_HEAD, "- test (failure)", a_minute_ago())
        .await
        .expect("hold");
    let next_id = repo
        .claim_hold_task(&base, next.since, Utc::now())
        .await
        .expect("claim next")
        .expect("claimed");
    assert_ne!(next_id, first);
}
