//! The board's read: whose Runs, where each one is, the filters, and the
//! pages.

use chrono::Utc;
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::test::{entry, julia, key, outcome, teo};
use crate::domain::board::{BoardCursor, BoardQuery, BoardState, RunBoardRepository};
use crate::domain::main_hold::MainHoldRepository;
use crate::domain::models::{NewRun, Repository, RunId, RunInsert, RunOutcome};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;
use crate::outbound::pg_run_repository::PgRunRepository;

const TEAM: &str = "00000000-0000-7000-8000-000000000201";
const LOGIN_TASK: &str = "00000000-0000-7000-8000-000000000101";
const CHECKOUT: &str = "00000000-0000-7000-8000-000000000301";
const REPO: &str = "https://github.com/acme/shop";

/// What a test Run is kept with.
struct Kept<'a> {
    user_id: MacroUserIdStr<'static>,
    flow: &'a str,
    request: &'a str,
    task_id: Option<&'a str>,
    project_id: Option<&'a str>,
    repo_url: Option<&'a str>,
}

impl<'a> Kept<'a> {
    fn julias(request: &'a str) -> Self {
        Kept {
            user_id: julia(),
            flow: "plan",
            request,
            task_id: None,
            project_id: None,
            repo_url: None,
        }
    }
}

async fn keep(repo: &PgRunRepository, kept: Kept<'_>) -> RunId {
    let id = RunId::generate();
    let repository = kept.repo_url.map(|url| Repository {
        url: url.to_string(),
        base_branch: "main".to_string(),
    });
    let inserted = repo
        .insert_run(NewRun {
            id,
            flow: kept.flow,
            request: kept.request,
            user_id: &kept.user_id,
            task_id: kept
                .task_id
                .map(|task| Uuid::parse_str(task).expect("task id")),
            project_id: kept
                .project_id
                .map(|project| Uuid::parse_str(project).expect("project id")),
            repository: repository.as_ref(),
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
        .await
        .expect("insert");
    assert!(matches!(inserted, RunInsert::Inserted(_)), "kept");
    id
}

fn query(team_id: Option<&str>) -> BoardQuery {
    BoardQuery {
        user_id: julia(),
        team_id: team_id.map(|team| Uuid::parse_str(team).expect("team id")),
        state: None,
        flow: None,
        repo_url: None,
        project_id: None,
        search: None,
        merge_entries: vec!["merge".to_string()],
        after: None,
        limit: 50,
    }
}

async fn read(repo: &PgRunRepository, query: &BoardQuery) -> Vec<(RunId, BoardState)> {
    repo.board_runs(query)
        .await
        .expect("board")
        .into_iter()
        .map(|listed| (listed.run.id, listed.state))
        .collect()
}

/// Each Run is where it is: running, waiting on its owner at an open Gate,
/// held at its merge while its base is red, stopped for a person, or
/// finished; newest first, and each state reads alone.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn each_run_is_where_it_is(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let finished = keep(&repo, Kept::julias("Ship the banner")).await;
    repo.end_run(finished, RunOutcome::Success)
        .await
        .expect("end");
    let stopped = keep(&repo, Kept::julias("Fix the flaky test")).await;
    repo.end_run(stopped, RunOutcome::Failed)
        .await
        .expect("end");
    let waiting = keep(&repo, Kept::julias("Plan the migration")).await;
    repo.start_step(key(waiting, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.open_gate(
        key(waiting, 1),
        "Approve the plan?",
        &[outcome("approve"), outcome("revise")],
    )
    .await
    .expect("open");
    let held = keep(
        &repo,
        Kept {
            flow: "code",
            repo_url: Some(REPO),
            ..Kept::julias("Add the coupon field")
        },
    )
    .await;
    repo.start_step(key(held, 1), &entry("merge"), 1)
        .await
        .expect("start");
    let base = Repository {
        url: REPO.to_string(),
        base_branch: "main".to_string(),
    };
    repo.hold_main(&base, &"a".repeat(40), "build: failed", Utc::now())
        .await
        .expect("hold");
    // A Run on the held base that is not at its merge goes on.
    let running = keep(
        &repo,
        Kept {
            flow: "code",
            repo_url: Some(REPO),
            ..Kept::julias("Rename the cart")
        },
    )
    .await;
    repo.start_step(key(running, 1), &entry("write"), 1)
        .await
        .expect("start");

    assert_eq!(
        read(&repo, &query(None)).await,
        [
            (running, BoardState::Running),
            (held, BoardState::Held),
            (waiting, BoardState::Waiting),
            (stopped, BoardState::Stopped),
            (finished, BoardState::Finished),
        ]
    );
    for (id, state) in [
        (running, BoardState::Running),
        (held, BoardState::Held),
        (waiting, BoardState::Waiting),
        (stopped, BoardState::Stopped),
        (finished, BoardState::Finished),
    ] {
        let only = BoardQuery {
            state: Some(state),
            ..query(None)
        };
        assert_eq!(read(&repo, &only).await, [(id, state)], "{state:?}");
    }

    // Once the base is green again, the Run at its merge goes on.
    repo.clear_main_hold(&base, Utc::now())
        .await
        .expect("clear");
    let held_now = BoardQuery {
        state: Some(BoardState::Held),
        ..query(None)
    };
    assert!(read(&repo, &held_now).await.is_empty());
}

/// A person's board lists their own Runs; their team's lists the team
/// members' too.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn the_team_board_lists_the_members_runs(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julias = keep(&repo, Kept::julias("Plan the migration")).await;
    let teos = keep(
        &repo,
        Kept {
            user_id: teo(),
            ..Kept::julias("Plan the rollback")
        },
    )
    .await;

    let ids = |listed: Vec<(RunId, BoardState)>| -> Vec<RunId> {
        listed.into_iter().map(|(id, _)| id).collect()
    };
    assert_eq!(ids(read(&repo, &query(None)).await), [julias]);
    assert_eq!(ids(read(&repo, &query(Some(TEAM))).await), [teos, julias]);
    // Another team's id lists none of its members' Runs, only the person's.
    let other_team = Uuid::now_v7().to_string();
    assert_eq!(ids(read(&repo, &query(Some(&other_team))).await), [julias]);
}

/// The Flow, repository, Project and search each narrow the board; the
/// search reads the request and the task's name, its `%` literally.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "tasks"))
)]
async fn the_filters_narrow_the_board(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let on_task = keep(
        &repo,
        Kept {
            task_id: Some(LOGIN_TASK),
            project_id: Some(CHECKOUT),
            ..Kept::julias("Look into it")
        },
    )
    .await;
    let coded = keep(
        &repo,
        Kept {
            flow: "code",
            repo_url: Some(REPO),
            ..Kept::julias("Cut prices by 10%")
        },
    )
    .await;
    let planned = keep(&repo, Kept::julias("Cut prices by 100")).await;

    let ids = |listed: Vec<(RunId, BoardState)>| -> Vec<RunId> {
        listed.into_iter().map(|(id, _)| id).collect()
    };
    let by_flow = read(
        &repo,
        &BoardQuery {
            flow: Some("code".to_string()),
            ..query(None)
        },
    )
    .await;
    assert_eq!(ids(by_flow), [coded]);
    let by_repo = read(
        &repo,
        &BoardQuery {
            repo_url: Some(REPO.to_string()),
            ..query(None)
        },
    )
    .await;
    assert_eq!(ids(by_repo), [coded]);
    let by_project = read(
        &repo,
        &BoardQuery {
            project_id: Some(Uuid::parse_str(CHECKOUT).expect("project")),
            ..query(None)
        },
    )
    .await;
    assert_eq!(ids(by_project), [on_task]);
    let by_task_name = read(
        &repo,
        &BoardQuery {
            search: Some("LOGIN bug".to_string()),
            ..query(None)
        },
    )
    .await;
    assert_eq!(ids(by_task_name), [on_task]);
    let by_request = read(
        &repo,
        &BoardQuery {
            search: Some("prices by 10%".to_string()),
            ..query(None)
        },
    )
    .await;
    assert_eq!(ids(by_request), [coded]);
    let by_words = read(
        &repo,
        &BoardQuery {
            search: Some("cut prices".to_string()),
            ..query(None)
        },
    )
    .await;
    assert_eq!(ids(by_words), [planned, coded]);
}

/// A page starts past the Run the page before ended on, and stops at its
/// limit.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_page_starts_past_the_last_one(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let mut kept = Vec::new();
    for request in ["One", "Two", "Three", "Four", "Five"] {
        kept.push(keep(&repo, Kept::julias(request)).await);
    }
    kept.reverse();

    let first = repo
        .board_runs(&BoardQuery {
            limit: 2,
            ..query(None)
        })
        .await
        .expect("board");
    let first_ids: Vec<RunId> = first.iter().map(|listed| listed.run.id).collect();
    assert_eq!(first_ids, kept[..2]);

    let last = first.last().expect("a run");
    let second = repo
        .board_runs(&BoardQuery {
            limit: 2,
            after: Some(BoardCursor {
                created_at: last.run.created_at,
                id: last.run.id,
            }),
            ..query(None)
        })
        .await
        .expect("board");
    let second_ids: Vec<RunId> = second.iter().map(|listed| listed.run.id).collect();
    assert_eq!(second_ids, kept[2..4]);
}
