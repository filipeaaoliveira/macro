//! The board: a person's own Runs, their team's on tasks they can see, the
//! pages, and who may read a team member's Run.

use factory_records::domain::board::BoardState;
use factory_records::domain::models::{
    EntryId, NewRun, RunId, RunInsert, RunOutcome, StepKey, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FakeWorkflows, julia, service, teo};
use crate::domain::board::{BoardFilter, BoardScope};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

const TEAM: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0201);

fn ana() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|ana@example.com".to_string()).expect("valid")
}

fn on_team(repo: &FakeRepo, user_id: &MacroUserIdStr<'static>) {
    repo.teams.lock().expect("teams").insert(
        user_id.as_ref().to_string(),
        TeamMembership {
            team_id: TEAM,
            admin: false,
        },
    );
}

fn task(repo: &FakeRepo) -> Uuid {
    let task_id = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task_id);
    task_id
}

fn hide(repo: &FakeRepo, user_id: &MacroUserIdStr<'static>, task_id: Uuid) {
    repo.hidden
        .lock()
        .expect("hidden")
        .insert((user_id.as_ref().to_string(), task_id));
}

async fn keep(repo: &FakeRepo, owner: &MacroUserIdStr<'static>, task_id: Option<Uuid>) -> RunId {
    keep_asking(repo, owner, task_id, "Plan the migration").await
}

async fn keep_asking(
    repo: &FakeRepo,
    owner: &MacroUserIdStr<'static>,
    task_id: Option<Uuid>,
    request: &str,
) -> RunId {
    let id = RunId::generate();
    let inserted = repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request,
            user_id: owner,
            task_id,
            project_id: None,
            repository: None,
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

fn filter(scope: BoardScope) -> BoardFilter {
    BoardFilter {
        scope,
        state: None,
        flow: None,
        repo_url: None,
        project_id: None,
        search: None,
        after: None,
        limit: 25,
    }
}

fn ids(page: &crate::domain::board::BoardPage) -> Vec<RunId> {
    page.runs
        .iter()
        .map(|listed| listed.detail.run.id)
        .collect()
}

/// A person's board lists their own Runs only; their team's lists the
/// members' Runs on tasks they can see, never one with no task or on a task
/// hidden from them, and nobody's off the team.
#[tokio::test]
async fn the_team_board_lists_teammates_runs_on_tasks_the_person_sees() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());

    let own = keep(&repo, &julia(), None).await;
    let shared_task = task(&repo);
    let shared = keep(&repo, &teo(), Some(shared_task)).await;
    let _no_task = keep(&repo, &teo(), None).await;
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);
    let _hidden = keep(&repo, &teo(), Some(hidden_task)).await;
    let _off_team = keep(&repo, &ana(), Some(shared_task)).await;

    let mine = service
        .board(&julia(), filter(BoardScope::Mine))
        .await
        .expect("board");
    assert_eq!(ids(&mine), [own]);

    let team = service
        .board(&julia(), filter(BoardScope::Team))
        .await
        .expect("board");
    assert_eq!(ids(&team), [shared, own]);
    assert!(
        team.runs
            .iter()
            .all(|listed| listed.state == BoardState::Running)
    );
}

/// A page names where the next one starts only while a Run the person sees
/// follows, past the last Run it lists: a team member's Run they cannot see
/// never shapes it.
#[tokio::test]
async fn a_page_names_where_the_next_starts_past_the_last_run_listed() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);

    let _oldest_hidden = keep(&repo, &teo(), Some(hidden_task)).await;
    let oldest = keep(&repo, &julia(), None).await;
    let _hidden = keep(&repo, &teo(), Some(hidden_task)).await;
    let middle = keep(&repo, &julia(), None).await;
    let newest = keep(&repo, &julia(), None).await;
    let page = |after| BoardFilter {
        limit: 2,
        after,
        ..filter(BoardScope::Team)
    };

    let first = service.board(&julia(), page(None)).await.expect("board");
    assert_eq!(ids(&first), [newest, middle]);
    let next = first.next.expect("another page");
    assert_eq!(next.id, middle);

    let second = service
        .board(&julia(), page(Some(next)))
        .await
        .expect("board");
    assert_eq!(ids(&second), [oldest]);
    // Only a hidden Run is older: the page is the last.
    assert!(second.next.is_none());
}

/// A search that only a team member's hidden Run matches reads exactly as
/// one nothing matches: no Run, and no next page.
#[tokio::test]
async fn a_search_only_hidden_runs_match_reads_as_no_match() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);
    let _own = keep(&repo, &julia(), None).await;
    let _hidden = keep_asking(&repo, &teo(), Some(hidden_task), "Rotate the secret key").await;
    let search = |words: &str| BoardFilter {
        limit: 1,
        search: Some(words.to_string()),
        ..filter(BoardScope::Team)
    };

    let hidden_match = service
        .board(&julia(), search("secret"))
        .await
        .expect("board");
    let no_match = service
        .board(&julia(), search("nothing like it"))
        .await
        .expect("board");

    assert!(hidden_match.runs.is_empty());
    assert!(hidden_match.next.is_none());
    assert_eq!(hidden_match, no_match);
}

/// Past many hidden Runs, the page still lists the Runs the person sees
/// behind them, asking which tasks they see once a round.
#[tokio::test]
async fn a_page_reads_past_hidden_runs() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);
    let own = keep(&repo, &julia(), None).await;
    for _ in 0..60 {
        keep(&repo, &teo(), Some(hidden_task)).await;
    }

    let page = service
        .board(
            &julia(),
            BoardFilter {
                limit: 1,
                ..filter(BoardScope::Team)
            },
        )
        .await
        .expect("board");

    assert_eq!(ids(&page), [own]);
    assert!(page.next.is_none());
    // Two rounds, one check: the second round's task was decided.
    assert_eq!(*repo.view_checks.lock().expect("checks"), 1);
}

/// Pages of two across rounds of hidden Runs list every Run the person
/// sees once, newest first.
#[tokio::test]
async fn pages_across_rounds_skip_and_repeat_no_run() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let mut own = Vec::new();
    for _ in 0..4 {
        own.push(keep(&repo, &julia(), None).await);
        for _ in 0..30 {
            let hidden_task = task(&repo);
            hide(&repo, &julia(), hidden_task);
            keep(&repo, &teo(), Some(hidden_task)).await;
        }
    }
    own.push(keep(&repo, &julia(), None).await);
    own.reverse();

    let mut listed = Vec::new();
    let mut after = None;
    loop {
        let page = service
            .board(
                &julia(),
                BoardFilter {
                    limit: 2,
                    after,
                    ..filter(BoardScope::Team)
                },
            )
            .await
            .expect("board");
        listed.extend(ids(&page));
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }

    assert_eq!(listed, own);
}

/// More hidden Runs in a row than the rounds read end the page there:
/// the older Run behind them is not listed, and no next page is named.
#[tokio::test]
async fn the_rounds_end_a_page_past_some_250_hidden_runs() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);
    let _behind = keep(&repo, &julia(), None).await;
    // Five rounds of fifty read 250 Runs.
    for _ in 0..260 {
        keep(&repo, &teo(), Some(hidden_task)).await;
    }

    let page = service
        .board(
            &julia(),
            BoardFilter {
                limit: 1,
                ..filter(BoardScope::Team)
            },
        )
        .await
        .expect("board");

    assert!(page.runs.is_empty());
    assert!(page.next.is_none());
}

/// A board read again soon after asks no task view again.
#[tokio::test]
async fn a_board_read_again_soon_asks_no_task_view_again() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let shared_task = task(&repo);
    keep(&repo, &teo(), Some(shared_task)).await;

    for _ in 0..2 {
        let page = service
            .board(&julia(), filter(BoardScope::Team))
            .await
            .expect("board");
        assert_eq!(page.runs.len(), 1);
    }

    assert_eq!(*repo.view_checks.lock().expect("checks"), 1);
}

/// A team member's Run on a task that is no longer there is hidden.
#[tokio::test]
async fn a_run_on_a_trashed_task_is_hidden() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let gone = task(&repo);
    keep(&repo, &teo(), Some(gone)).await;
    repo.tasks.lock().expect("tasks").remove(&gone);

    let page = service
        .board(&julia(), filter(BoardScope::Team))
        .await
        .expect("board");

    assert!(page.runs.is_empty());
}

/// A team member's read of a Run whose workflow never started changes
/// nothing; its owner's read ends it.
#[tokio::test]
async fn only_the_owner_s_read_ends_a_run_that_never_started() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = service(&repo, &workflows);
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let shared_task = task(&repo);
    let run_id = keep(&repo, &teo(), Some(shared_task)).await;
    if let Some(run) = repo.runs.lock().expect("runs").get_mut(&run_id) {
        run.created_at -= chrono::TimeDelta::hours(1);
    }

    let read = service.get_run(&julia(), run_id).await.expect("read");
    assert!(read.run.end.is_none());

    let owner_s = service.get_run(&teo(), run_id).await.expect("read");
    assert!(owner_s.run.end.is_some());
}

/// The board says where each Run is, and lists those of one state alone.
#[tokio::test]
async fn the_board_says_where_each_run_is() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    let finished = keep(&repo, &julia(), None).await;
    repo.end_run(finished, RunOutcome::Success)
        .await
        .expect("end");
    let stopped = keep(&repo, &julia(), None).await;
    repo.end_run(stopped, RunOutcome::Blocked)
        .await
        .expect("end");
    let waiting = keep(&repo, &julia(), None).await;
    let key = StepKey {
        run_id: waiting,
        seq: 1,
    };
    let entry = EntryId::try_from("plan".to_string()).expect("entry");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.open_gate(key, "Approve the plan?", &[])
        .await
        .expect("open");

    let page = service
        .board(&julia(), filter(BoardScope::Mine))
        .await
        .expect("board");
    let states: Vec<(RunId, BoardState)> = page
        .runs
        .iter()
        .map(|listed| (listed.detail.run.id, listed.state))
        .collect();
    assert_eq!(
        states,
        [
            (waiting, BoardState::Waiting),
            (stopped, BoardState::Stopped),
            (finished, BoardState::Finished),
        ]
    );

    let only = service
        .board(
            &julia(),
            BoardFilter {
                state: Some(BoardState::Stopped),
                ..filter(BoardScope::Mine)
            },
        )
        .await
        .expect("board");
    assert_eq!(ids(&only), [stopped]);
}

/// A team member reads a Run on a task they can see, as the board opens
/// it; the Run stays its owner's to act on, and one with no task, on a
/// hidden task or off the team stays unknown to them.
#[tokio::test]
async fn a_team_member_reads_a_run_on_a_task_they_see() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let shared_task = task(&repo);
    let shared = keep(&repo, &teo(), Some(shared_task)).await;
    let no_task = keep(&repo, &teo(), None).await;
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);
    let hidden = keep(&repo, &teo(), Some(hidden_task)).await;
    let off_team = keep(&repo, &ana(), Some(shared_task)).await;

    let read = service.get_run(&julia(), shared).await.expect("read");
    assert_eq!(read.run.user_id, teo());
    for id in [no_task, hidden, off_team] {
        let refused = service.get_run(&julia(), id).await;
        assert!(matches!(refused, Err(FactoryError::NotFound)), "{id}");
    }
    let cancel = service.cancel_run(&julia(), shared).await;
    assert!(matches!(cancel, Err(FactoryError::NotFound)));
}

/// A Run keeps the Project its task is in when it starts, which the board
/// filters by.
#[tokio::test]
async fn a_run_keeps_the_project_its_task_is_in() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    let task_id = task(&repo);
    let project = Uuid::now_v7();
    repo.task_projects
        .lock()
        .expect("task projects")
        .insert(task_id, project);

    let created = service
        .create_run(
            julia(),
            "reply".to_string(),
            "Summarise the release notes".to_string(),
            Some(task_id),
            None,
            true,
        )
        .await
        .expect("create");

    let kept = repo
        .get_run(created.run.id)
        .await
        .expect("get")
        .expect("kept");
    assert_eq!(kept.project_id, Some(project));
}
