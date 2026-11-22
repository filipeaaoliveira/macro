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

/// Runs that reached their merge in the same instant keep the order they
/// entered the queue in, whatever their ids.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn runs_that_entered_in_the_same_instant_keep_their_order(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let main = branch_of_app("main");
    // Ids sort in the order they are made: the later one enters first.
    let earlier_id = code_run(&repo, Some(&main)).await;
    let later_id = code_run(&repo, Some(&main)).await;
    repo.enter_merge_queue(later_id).await.expect("enter");
    repo.enter_merge_queue(earlier_id).await.expect("enter");
    sqlx::query!("UPDATE factory_merge_queue SET entered_at = '2026-11-19T04:00:00Z'")
        .execute(&repo.pool)
        .await
        .expect("same instant");

    assert_eq!(ahead(&repo, later_id).await, Vec::<RunId>::new());
    assert_eq!(ahead(&repo, earlier_id).await, vec![later_id]);
}

/// One repository is one queue, whatever the case a Run named it in.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_repository_named_in_another_case_is_one_queue(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let first = code_run(&repo, Some(&branch_of_app("main"))).await;
    let shouted = Repository {
        url: "https://github.com/Macro/App".to_string(),
        base_branch: "main".to_string(),
    };
    let second = code_run(&repo, Some(&shouted)).await;
    repo.enter_merge_queue(first).await.expect("enter");
    repo.enter_merge_queue(second).await.expect("enter");

    assert_eq!(ahead(&repo, second).await, vec![first]);
}

/// One Run of a base holds its merge turn at a time: another Run of that
/// base, in whatever case its repository is named, takes none until the
/// turn is dropped.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn two_runs_on_one_base_take_its_merge_turn_one_at_a_time(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let first = code_run(&repo, Some(&branch_of_app("main"))).await;
    let shouting = Repository {
        url: "https://github.com/Macro/App.git".to_string(),
        base_branch: "main".to_string(),
    };
    let second = code_run(&repo, Some(&shouting)).await;

    let turn = repo.take_merge_turn(first).await.expect("take");
    assert!(turn.is_some(), "the first Run takes the turn");
    assert!(
        repo.take_merge_turn(second).await.expect("take").is_none(),
        "the base's turn is held"
    );

    // Dropping the turn rolls its transaction back as the connection goes
    // back to the pool, which lets the lock go.
    drop(turn);
    let mut taken = None;
    for _ in 0..50 {
        taken = repo.take_merge_turn(second).await.expect("take");
        if taken.is_some() {
            break;
        }
        // A wait that yields, so the pool's return of the connection runs.
        sqlx::query!(r#"SELECT pg_sleep(0.1)::text AS "slept""#)
            .fetch_one(&repo.pool)
            .await
            .expect("wait");
    }
    assert!(taken.is_some(), "a dropped turn is let go");
}

/// Two Runs on different bases each take their own merge turn at once: a
/// Run of one repository's `release` and one of another repository's
/// `main` wait on no `main` turn of this repository.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn two_runs_on_different_bases_take_their_turns_at_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let main = code_run(&repo, Some(&branch_of_app("main"))).await;
    let release = code_run(&repo, Some(&branch_of_app("release"))).await;
    let other_repository = Repository {
        url: "https://github.com/macro/site".to_string(),
        base_branch: "main".to_string(),
    };
    let elsewhere = code_run(&repo, Some(&other_repository)).await;

    let main_turn = repo.take_merge_turn(main).await.expect("take");
    assert!(main_turn.is_some(), "the main Run takes its turn");
    let release_turn = repo.take_merge_turn(release).await.expect("take");
    assert!(release_turn.is_some(), "another branch's turn is its own");
    let elsewhere_turn = repo.take_merge_turn(elsewhere).await.expect("take");
    assert!(
        elsewhere_turn.is_some(),
        "another repository's turn is its own"
    );
}

/// A held merge turn locks no row or table of any factory table: the Run
/// it is for can be written, and its table locked whole, as by a
/// migration, while it is held.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_held_merge_turn_locks_no_factory_table(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run = code_run(&repo, Some(&branch_of_app("main"))).await;
    let turn = repo.take_merge_turn(run).await.expect("take");
    assert!(turn.is_some(), "the Run takes its turn");

    let held = sqlx::query_scalar!(
        r#"
        SELECT c.relname AS "relname!"
        FROM pg_locks turn
        JOIN pg_locks held ON held.pid = turn.pid
        JOIN pg_class c ON c.oid = held.relation
        WHERE turn.locktype = 'advisory' AND turn.granted
            AND turn.database = (SELECT oid FROM pg_database WHERE datname = current_database())
            AND held.locktype = 'relation'
            AND held.database = turn.database
            AND c.relname LIKE 'factory%'
        "#,
    )
    .fetch_all(&repo.pool)
    .await
    .expect("locks");
    assert_eq!(held, Vec::<String>::new(), "the turn holds factory locks");

    let mut other = repo.pool.begin().await.expect("begin");
    sqlx::query!("LOCK TABLE factory_run IN ACCESS EXCLUSIVE MODE NOWAIT")
        .execute(&mut *other)
        .await
        .expect("the Run's table is not locked by the turn");
    sqlx::query!(
        "UPDATE factory_run SET merge_head_sha = merge_head_sha WHERE id = $1",
        run.as_uuid(),
    )
    .execute(&mut *other)
    .await
    .expect("the Run's row is not locked by the turn");
    other.rollback().await.expect("rollback");
    drop(turn);
}

/// A base's merge turn is keyed by the same advisory lock key in every
/// release: workers on either side of a deploy share one turn per base only
/// while the key stays the same. The literal is
/// `hashtextextended('factory_merge_turn:https://github.com/macro/app main', 0)`,
/// the key earlier workers took for this base.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_merge_turn_keeps_the_key_earlier_workers_took(pool: PgPool) {
    const MACRO_APP_MAIN_KEY: i64 = -5_814_030_250_524_603_323;
    let repo = PgRunRepository::new(pool);
    let shouting = Repository {
        url: "https://github.com/Macro/App.git".to_string(),
        base_branch: "main".to_string(),
    };
    let run = code_run(&repo, Some(&shouting)).await;
    let turn = repo.take_merge_turn(run).await.expect("take");
    assert!(turn.is_some(), "the Run takes its turn");

    let mut earlier_worker = repo.pool.begin().await.expect("begin");
    let taken = sqlx::query_scalar!(
        r#"SELECT pg_try_advisory_xact_lock($1) AS "taken!""#,
        MACRO_APP_MAIN_KEY,
    )
    .fetch_one(&mut *earlier_worker)
    .await
    .expect("try");
    assert!(!taken, "the turn holds the key earlier workers took");
    earlier_worker.rollback().await.expect("rollback");
    drop(turn);
}

/// The head a Run's review passed on is kept until another replaces it;
/// asking an update again from a pushed head never touches it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_reviewed_head_is_kept_until_replaced(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = code_run(&repo, Some(&branch_of_app("main"))).await;
    assert_eq!(repo.reviewed_head(run_id).await.expect("read"), None);

    repo.set_reviewed_head(run_id, HEAD).await.expect("set");
    let key = StepKey { run_id, seq: 9 };
    repo.keep_branch_update(
        key,
        &BranchUpdate {
            base_sha: BASE.to_string(),
            from_head: HEAD.to_string(),
            conflict: false,
            updated_head: None,
        },
    )
    .await
    .expect("keep");
    let pushed = "3333333333333333333333333333333333333333";
    repo.restart_branch_update(
        key,
        &BranchUpdate {
            base_sha: BASE.to_string(),
            from_head: pushed.to_string(),
            conflict: false,
            updated_head: None,
        },
    )
    .await
    .expect("restart");
    assert_eq!(
        repo.reviewed_head(run_id).await.expect("read").as_deref(),
        Some(HEAD)
    );

    repo.set_reviewed_head(run_id, pushed).await.expect("set");
    assert_eq!(
        repo.reviewed_head(run_id).await.expect("read").as_deref(),
        Some(pushed)
    );
}
