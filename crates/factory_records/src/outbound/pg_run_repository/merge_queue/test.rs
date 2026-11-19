use chrono::TimeDelta;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use super::super::PgRunRepository;
use crate::domain::merge_queue::{BranchUpdate, MergeQueueRepository, QUEUE_PLACE_KEPT};
use crate::domain::models::{NewRun, Repository, RunId, RunInsert, RunOutcome, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;

const BASE: &str = "2222222222222222222222222222222222222222";
const HEAD: &str = "1111111111111111111111111111111111111111";

fn branch_of_app(branch: &str) -> Repository {
    Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: branch.to_string(),
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
            project_id: None,
            repository,
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
    id
}

async fn ahead(repo: &PgRunRepository, run_id: RunId) -> Vec<RunId> {
    repo.merge_queue_ahead(run_id, QUEUE_PLACE_KEPT)
        .await
        .expect("ahead")
}

/// Runs merge to a base in the order they first reached their merge: each
/// waits for those before it, a Run on another base waits for none, and a
/// Run that merged no longer holds anyone.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn runs_merge_to_a_base_in_the_order_they_reached_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let main = branch_of_app("main");
    let first = code_run(&repo, Some(&main)).await;
    let second = code_run(&repo, Some(&main)).await;
    let elsewhere = code_run(&repo, Some(&branch_of_app("release"))).await;

    repo.enter_merge_queue(first).await.expect("enter");
    repo.enter_merge_queue(second).await.expect("enter");
    repo.enter_merge_queue(elsewhere).await.expect("enter");
    // Entering again keeps the place.
    repo.enter_merge_queue(first).await.expect("enter again");

    assert_eq!(ahead(&repo, first).await, Vec::<RunId>::new());
    assert_eq!(ahead(&repo, second).await, vec![first]);
    assert_eq!(ahead(&repo, elsewhere).await, Vec::<RunId>::new());

    repo.drop_from_merge_queue(first).await.expect("drop");
    assert_eq!(ahead(&repo, second).await, Vec::<RunId>::new());
}

/// A Run that left its merge to update its branch keeps its place for a
/// while, and comes back to it; one that ended holds no one.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_away_from_its_merge_keeps_its_place_for_a_while(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let main = branch_of_app("main");
    let first = code_run(&repo, Some(&main)).await;
    let second = code_run(&repo, Some(&main)).await;
    repo.enter_merge_queue(first).await.expect("enter");
    repo.enter_merge_queue(second).await.expect("enter");

    repo.leave_merge_queue(first).await.expect("leave");
    assert_eq!(ahead(&repo, second).await, vec![first]);
    // Gone longer than its place is kept.
    let none = repo
        .merge_queue_ahead(second, TimeDelta::zero())
        .await
        .expect("ahead");
    assert_eq!(none, Vec::<RunId>::new());

    // Back at its merge, it is first again.
    repo.enter_merge_queue(first).await.expect("back");
    assert_eq!(
        repo.merge_queue_ahead(second, TimeDelta::zero())
            .await
            .expect("ahead"),
        vec![first]
    );

    repo.end_run(first, RunOutcome::Manual).await.expect("end");
    assert_eq!(ahead(&repo, second).await, Vec::<RunId>::new());
    let left = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM factory_merge_queue WHERE run_id = $1"#,
        first.as_uuid(),
    )
    .fetch_one(&repo.pool)
    .await
    .expect("count");
    assert_eq!(left, 0, "an ended run leaves its base's queue");
}

/// A Run with no repository joins no queue.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_with_no_repository_joins_no_queue(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run = code_run(&repo, None).await;
    repo.enter_merge_queue(run).await.expect("enter");
    assert_eq!(ahead(&repo, run).await, Vec::<RunId>::new());
}

/// A Step keeps its first branch update, and the head GitHub's merge made
/// only for an update with no conflict.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_step_keeps_its_first_branch_update(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = code_run(&repo, Some(&branch_of_app("main"))).await;
    let key = StepKey { run_id, seq: 9 };
    let other = StepKey { run_id, seq: 10 };
    assert_eq!(repo.branch_update(key).await.expect("read"), None);

    let clean = BranchUpdate {
        base_sha: BASE.to_string(),
        from_head: HEAD.to_string(),
        conflict: false,
        updated_head: None,
    };
    let kept = repo.keep_branch_update(key, &clean).await.expect("keep");
    assert_eq!(kept, clean);
    let again = repo
        .keep_branch_update(
            key,
            &BranchUpdate {
                conflict: true,
                ..clean.clone()
            },
        )
        .await
        .expect("keep again");
    assert_eq!(again, clean);

    let merged = "3333333333333333333333333333333333333333";
    repo.set_updated_head(key, merged).await.expect("head");
    assert_eq!(
        repo.branch_update(key).await.expect("read"),
        Some(BranchUpdate {
            updated_head: Some(merged.to_string()),
            ..clean.clone()
        })
    );

    let conflict = BranchUpdate {
        conflict: true,
        ..clean
    };
    repo.keep_branch_update(other, &conflict)
        .await
        .expect("keep");
    repo.set_updated_head(other, merged).await.expect("head");
    assert_eq!(
        repo.branch_update(other).await.expect("read"),
        Some(conflict)
    );
}

/// An update asked again replaces one whose merge never showed, and never
/// one that is a conflict or whose merge showed.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn an_update_asked_again_replaces_only_one_still_waiting(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = code_run(&repo, Some(&branch_of_app("main"))).await;
    let waiting = StepKey { run_id, seq: 9 };
    let shown = StepKey { run_id, seq: 10 };
    let conflict = StepKey { run_id, seq: 11 };
    let started = BranchUpdate {
        base_sha: BASE.to_string(),
        from_head: HEAD.to_string(),
        conflict: false,
        updated_head: None,
    };
    let pushed = "4444444444444444444444444444444444444444";
    let again = BranchUpdate {
        from_head: pushed.to_string(),
        ..started.clone()
    };
    for key in [waiting, shown] {
        repo.keep_branch_update(key, &started).await.expect("keep");
    }
    let merged = "3333333333333333333333333333333333333333";
    repo.set_updated_head(shown, merged).await.expect("head");
    let conflicted = BranchUpdate {
        conflict: true,
        ..started.clone()
    };
    repo.keep_branch_update(conflict, &conflicted)
        .await
        .expect("keep");

    assert_eq!(
        repo.restart_branch_update(waiting, &again)
            .await
            .expect("again"),
        again
    );
    assert_eq!(
        repo.restart_branch_update(shown, &again)
            .await
            .expect("again"),
        BranchUpdate {
            updated_head: Some(merged.to_string()),
            ..started.clone()
        }
    );
    assert_eq!(
        repo.restart_branch_update(conflict, &again)
            .await
            .expect("again"),
        conflicted
    );
}
