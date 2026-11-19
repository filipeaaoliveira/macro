use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use super::super::PgRunRepository;
use crate::domain::merge_guard::{MergeGuardRepository, pull_request_page};
use crate::domain::models::{NewRun, RunId, RunInsert, RunOutcome};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;

/// A new Run of Julia's that opened the pull request at `page`.
async fn run_on(repo: &PgRunRepository, page: &str) -> RunId {
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
            repository: None,
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
    repo.set_pull_request_url(id, page).await.expect("page");
    id
}

/// A Run that has not ended holds its pull request, found by the page
/// GitHub names in any case; once it ends, it holds it no more.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_holds_its_pull_request_until_it_ends(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = run_on(&repo, "https://github.com/Acme/Shop/pull/7").await;
    let page = pull_request_page("acme", "shop", 7);

    let live = repo.live_run_of_pull_request(&page).await.expect("read");
    assert_eq!(live, Some(id));

    repo.end_run(id, RunOutcome::Blocked).await.expect("end");
    let ended = repo.live_run_of_pull_request(&page).await.expect("read");
    assert_eq!(ended, None);
}

/// A pull request no Run opened has no Run, though a Run holds another
/// pull request of its repository.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_pull_request_no_run_opened_has_none(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    run_on(&repo, "https://github.com/acme/shop/pull/7").await;

    let other = repo
        .live_run_of_pull_request(&pull_request_page("acme", "shop", 70))
        .await
        .expect("read");
    assert_eq!(other, None);
}

/// A pull request whose older Run ended and a newer one goes on is held by
/// the newer one.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_newer_run_holds_a_pull_request_an_older_one_left(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let page = pull_request_page("acme", "shop", 7);
    let older = run_on(&repo, &page).await;
    repo.end_run(older, RunOutcome::Manual).await.expect("end");
    let newer = run_on(&repo, &page).await;

    let live = repo.live_run_of_pull_request(&page).await.expect("read");
    assert_eq!(live, Some(newer));
}
