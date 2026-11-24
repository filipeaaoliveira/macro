//! Where an agent Step's session ran: the model and the Machine it opened
//! on, kept once it opens, and the display name of the agent whose session
//! it is. And when a Run's pull request was merged.

use chrono::{DurationRound, TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::{entry, insert, julia, key, outcome, planner};
use crate::domain::board::{BoardQuery, RunBoardRepository as _};
use crate::domain::main_hold::MainHoldRepository as _;
use crate::domain::models::{
    AgentName, NewRun, PassedOver, Repository, RunId, RunInsert, StepRuntime, StepSession,
};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::RunRules;
use crate::outbound::pg_run_repository::PgRunRepository;

/// Julia's agent bound to her Machine `m4` (`fixtures/agents.sql`).
const WRITER: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00b1);
/// Julia's agent on a runtime Macro runs, bound to no Machine.
const CLOUD: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00b2);
/// Her Machine `m3`.
const M3: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_00a2);

/// A new session of `bot`, as the Agent `name`.
fn session_on(name: &str, bot: Uuid) -> StepSession {
    StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(AgentName::try_from(name.to_string()).expect("agent")),
        bot: Some(bot),
    }
}

/// Open `session` in the agent session service, on `model`, as Julia's.
async fn open_session(pool: &PgPool, session: &StepSession, model: &str) {
    sqlx::query!(
        "INSERT INTO agent_session (id, owner_id, bot_id, model, harness, workspace)
         VALUES ($1, 'macro|julia@example.com', $2, $3, 'macrod', '/workspace')",
        session.session_id,
        session.bot.expect("a bot"),
        model,
    )
    .execute(pool)
    .await
    .expect("session");
}

async fn runtime(repo: &PgRunRepository, id: RunId, seq: u16) -> StepRuntime {
    repo.list_steps(id)
        .await
        .expect("steps")
        .into_iter()
        .find(|step| step.seq == seq)
        .expect("the step")
        .runtime
}

/// A Step keeps the model and Machine its session opened on, so they read
/// the same after the session is deleted and its agent is bound to another
/// Machine; its agent's name reads from the start.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn a_step_keeps_where_its_session_opened(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    insert(&repo, id, "Plan the export").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let session = session_on("primary", WRITER);
    repo.claim_session(key(id, 1), session.clone(), &planner(), &[outcome("done")])
        .await
        .expect("claim");

    assert_eq!(
        runtime(&repo, id, 1).await,
        StepRuntime {
            model: None,
            machine: None,
            agent_name: Some("Writer".to_string()),
        },
        "nothing ran yet, but the agent is known"
    );

    open_session(&pool, &session, "claude-opus-4-5").await;
    repo.record_open(key(id, 1), session.session_id)
        .await
        .expect("open");
    let opened = StepRuntime {
        model: Some("claude-opus-4-5".to_string()),
        machine: Some("m4".to_string()),
        agent_name: Some("Writer".to_string()),
    };
    assert_eq!(runtime(&repo, id, 1).await, opened);

    // The session goes once the Step is done with it, and the agent moves
    // to another Machine: the Step still says where it ran.
    sqlx::query!(
        "UPDATE agent_configs SET harness_id = $1 WHERE bot_id = $2",
        M3,
        WRITER,
    )
    .execute(&pool)
    .await
    .expect("rebind");
    sqlx::query!(
        "DELETE FROM agent_session WHERE id = $1",
        session.session_id
    )
    .execute(&pool)
    .await
    .expect("delete");
    repo.record_open(key(id, 1), session.session_id)
        .await
        .expect("open again");
    assert_eq!(runtime(&repo, id, 1).await, opened);

    // A list of Runs reads the same.
    let listed = repo.list_runs_steps(&[id]).await.expect("list");
    assert_eq!(listed[&id][0].runtime, opened);
}

/// A Step that goes on in a new session forgets where the one it left ran,
/// and reads where the new one opens: a runtime Macro runs is on no Machine.
/// A Step no agent does reads none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "agents"))
)]
async fn a_step_that_goes_on_in_a_new_session_reads_where_that_one_ran(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    insert(&repo, id, "Plan the export").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let first = session_on("primary", WRITER);
    repo.claim_session(key(id, 1), first.clone(), &planner(), &[outcome("done")])
        .await
        .expect("claim");
    open_session(&pool, &first, "claude-opus-4-5").await;
    repo.record_open(key(id, 1), first.session_id)
        .await
        .expect("open");

    let second = session_on("backup", CLOUD);
    let passed = PassedOver {
        agent: AgentName::try_from("primary".to_string()).expect("agent"),
        reason: "session_lost".to_string(),
    };
    assert!(
        repo.pass_session(
            key(id, 1),
            first.session_id,
            &planner(),
            &passed,
            Some(&second)
        )
        .await
        .expect("pass")
    );
    assert_eq!(
        runtime(&repo, id, 1).await,
        StepRuntime {
            model: None,
            machine: None,
            agent_name: Some("Cloud".to_string()),
        }
    );

    open_session(&pool, &second, "claude-sonnet-4-5").await;
    repo.record_open(key(id, 1), second.session_id)
        .await
        .expect("open");
    assert_eq!(
        runtime(&repo, id, 1).await,
        StepRuntime {
            model: Some("claude-sonnet-4-5".to_string()),
            machine: None,
            agent_name: Some("Cloud".to_string()),
        }
    );

    repo.start_step(key(id, 2), &entry("publish"), 1)
        .await
        .expect("start");
    assert_eq!(runtime(&repo, id, 2).await, StepRuntime::default());
}

/// A Run that merged its pull request reads when, on its own read and in
/// the board's; one that merged none reads none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_reads_when_its_pull_request_was_merged(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia: MacroUserIdStr<'static> = julia();
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    let mut ids = Vec::new();
    for _ in 0..2 {
        let id = RunId::generate();
        let RunInsert::Inserted(_) = repo
            .insert_run(NewRun {
                id,
                flow: "code",
                request: "Add dark mode",
                user_id: &julia,
                task_id: None,
                project_id: None,
                repository: Some(&repository),
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
        ids.push(id);
    }
    let (merged, open) = (ids[0], ids[1]);
    // Postgres keeps microseconds.
    let merged_at = (Utc::now() - TimeDelta::minutes(3))
        .duration_trunc(TimeDelta::microseconds(1))
        .expect("trunc");
    repo.record_merge(merged, 7, "https://github.com/macro/app/pull/7", merged_at)
        .await
        .expect("merge");

    let read = |id| {
        let repo = &repo;
        async move { repo.get_run(id).await.expect("get").expect("the run") }
    };
    assert_eq!(read(merged).await.merged_at, Some(merged_at));
    assert_eq!(read(open).await.merged_at, None);

    let board = repo
        .board_runs(&BoardQuery {
            user_id: julia.clone(),
            team_id: None,
            state: None,
            flow: None,
            repo_url: None,
            project_id: None,
            search: None,
            built_in_merge_entries: vec!["merge".to_string()],
            after: None,
            limit: 50,
        })
        .await
        .expect("board");
    assert_eq!(board.len(), 2);
    for listed in board {
        let expected = (listed.run.id == merged).then_some(merged_at);
        assert_eq!(listed.run.merged_at, expected);
    }
}
