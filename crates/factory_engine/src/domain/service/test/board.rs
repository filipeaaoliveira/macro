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
    let id = RunId::generate();
    let inserted = repo
        .insert_run(NewRun {
            id,
            flow: "plan",
            request: "Plan the migration",
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

/// A page names where the next one starts while more follow, past the
/// last Run it read even when that one was left out, so a page of hidden
/// Runs never repeats.
#[tokio::test]
async fn a_page_names_where_the_next_starts() {
    let repo = FakeRepo::default();
    let service = service(&repo, &FakeWorkflows::default());
    on_team(&repo, &julia());
    on_team(&repo, &teo());
    let hidden_task = task(&repo);
    hide(&repo, &julia(), hidden_task);

    let oldest = keep(&repo, &julia(), None).await;
    let _hidden = keep(&repo, &teo(), Some(hidden_task)).await;
    let newest = keep(&repo, &julia(), None).await;

    let first = service
        .board(
            &julia(),
            BoardFilter {
                limit: 2,
                ..filter(BoardScope::Team)
            },
        )
        .await
        .expect("board");
    assert_eq!(ids(&first), [newest]);
    let next = first.next.expect("another page");

    let second = service
        .board(
            &julia(),
            BoardFilter {
                limit: 2,
                after: Some(next),
                ..filter(BoardScope::Team)
            },
        )
        .await
        .expect("board");
    assert_eq!(ids(&second), [oldest]);
    assert!(second.next.is_none());
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
