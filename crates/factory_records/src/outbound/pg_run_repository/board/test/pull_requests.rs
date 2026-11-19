//! The Runs that opened some pull requests: whose, in what case, and where
//! each one is.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::super::test::{entry, julia, key, outcome, teo};
use super::{Kept, REPO, TEAM, keep};
use crate::domain::board::{
    BoardState, PullRequestRunsQuery, RunBoardRepository, pull_request_key,
};
use crate::domain::models::RunId;
use crate::domain::ports::RunRepository;
use crate::outbound::pg_run_repository::PgRunRepository;

const COUPON: &str = "https://github.com/Acme/Shop/pull/7";
const BANNER: &str = "https://github.com/acme/shop/pull/8";
const LOGIN_TASK: &str = "00000000-0000-7000-8000-000000000101";
const DARK_MODE_TASK: &str = "00000000-0000-7000-8000-000000000102";

fn query(team_id: Option<&str>, urls: &[&str]) -> PullRequestRunsQuery {
    PullRequestRunsQuery {
        user_id: julia(),
        team_id: team_id.map(|team| Uuid::parse_str(team).expect("team id")),
        urls: urls.iter().map(|url| pull_request_key(url)).collect(),
        merge_entries: vec!["merge".to_string()],
        limit: 50,
    }
}

async fn read(repo: &PgRunRepository, query: &PullRequestRunsQuery) -> Vec<(RunId, BoardState)> {
    repo.pull_request_runs(query)
        .await
        .expect("pull request runs")
        .into_iter()
        .map(|listed| (listed.run.id, listed.state))
        .collect()
}

/// A pull request's Runs are read in any case of its page, newest first,
/// with where each one is; a Run of a pull request not asked for, or with
/// none, is not.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../../fixtures", scripts("people"))
)]
async fn a_pull_request_s_runs_are_read_in_any_case(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let coupon = keep(
        &repo,
        Kept {
            flow: "code",
            repo_url: Some(REPO),
            ..Kept::julias("Add the coupon field")
        },
    )
    .await;
    repo.set_pull_request_url(coupon, COUPON)
        .await
        .expect("keep");
    repo.start_step(key(coupon, 1), &entry("merge-gate"), 1)
        .await
        .expect("start");
    repo.open_gate(
        key(coupon, 1),
        "Merge this pull request?",
        &[outcome("approve"), outcome("revise")],
    )
    .await
    .expect("open");
    let banner = keep(&repo, Kept::julias("Ship the banner")).await;
    repo.set_pull_request_url(banner, BANNER)
        .await
        .expect("keep");
    let _no_pull_request = keep(&repo, Kept::julias("Plan the rollback")).await;

    assert_eq!(
        read(
            &repo,
            &query(None, &["https://github.com/acme/shop/pull/7"])
        )
        .await,
        [(coupon, BoardState::Waiting)]
    );
    assert_eq!(
        read(&repo, &query(None, &[COUPON, BANNER])).await,
        [(banner, BoardState::Running), (coupon, BoardState::Waiting)]
    );
    assert!(
        read(
            &repo,
            &query(None, &["https://github.com/acme/shop/pull/9"])
        )
        .await
        .is_empty()
    );
}

/// The person's own Runs are read, and their team members' only when the
/// query names the team they are on.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../../fixtures", scripts("people", "team"))
)]
async fn a_team_member_s_run_is_read_only_for_their_team(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julias = keep(&repo, Kept::julias("Add the coupon field")).await;
    repo.set_pull_request_url(julias, COUPON)
        .await
        .expect("keep");
    let teos = keep(
        &repo,
        Kept {
            user_id: teo(),
            ..Kept::julias("Ship the banner")
        },
    )
    .await;
    repo.set_pull_request_url(teos, BANNER).await.expect("keep");

    let ids = |listed: Vec<(RunId, BoardState)>| -> Vec<RunId> {
        listed.into_iter().map(|(id, _)| id).collect()
    };
    assert_eq!(
        ids(read(&repo, &query(None, &[COUPON, BANNER])).await),
        [julias]
    );
    assert_eq!(
        ids(read(&repo, &query(Some(TEAM), &[COUPON, BANNER])).await),
        [teos, julias]
    );
    let other_team = Uuid::now_v7().to_string();
    assert_eq!(
        ids(read(&repo, &query(Some(&other_team), &[COUPON, BANNER])).await),
        [julias]
    );
}

/// Each pull request reads only the newest Run of each owner and task, so a
/// pile of one owner's Runs on one task never pushes an older Run of
/// another owner past the limit.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../../fixtures", scripts("people", "team", "tasks"))
)]
async fn a_pull_request_reads_the_newest_run_of_each_owner_and_task(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julias = keep(&repo, Kept::julias("Add the coupon field")).await;
    repo.set_pull_request_url(julias, COUPON)
        .await
        .expect("keep");
    let mut teos = julias;
    for _ in 0..5 {
        teos = keep(
            &repo,
            Kept {
                user_id: teo(),
                task_id: Some(LOGIN_TASK),
                ..Kept::julias("Fix the login bug")
            },
        )
        .await;
        repo.set_pull_request_url(teos, COUPON).await.expect("keep");
    }

    let ids: Vec<RunId> = read(
        &repo,
        &PullRequestRunsQuery {
            limit: 2,
            ..query(Some(TEAM), &[COUPON])
        },
    )
    .await
    .into_iter()
    .map(|(id, _)| id)
    .collect();
    assert_eq!(ids, [teos, julias]);
}

/// The limit holds for each pull request on its own: one pull request's
/// newer Runs never push another's past it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../../fixtures", scripts("people", "tasks"))
)]
async fn one_pull_request_s_runs_never_crowd_out_another_s(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let banner = keep(&repo, Kept::julias("Ship the banner")).await;
    repo.set_pull_request_url(banner, BANNER)
        .await
        .expect("keep");
    let mut coupons = Vec::new();
    for task_id in [None, Some(LOGIN_TASK), Some(DARK_MODE_TASK)] {
        let id = keep(
            &repo,
            Kept {
                task_id,
                ..Kept::julias("Add the coupon field")
            },
        )
        .await;
        repo.set_pull_request_url(id, COUPON).await.expect("keep");
        coupons.push(id);
    }

    let ids: Vec<RunId> = read(
        &repo,
        &PullRequestRunsQuery {
            limit: 2,
            ..query(None, &[COUPON, BANNER])
        },
    )
    .await
    .into_iter()
    .map(|(id, _)| id)
    .collect();
    assert_eq!(ids, [coupons[2], coupons[1], banner]);
}
