use agent_session::domain::routines::RoutineActionStatus;
use agent_session::domain::session_command::{SessionCommand, SessionCommandOutput};
use factory_records::domain::models::Role;
use factory_records::domain::ports::RunRepository;

use super::{FakeSessions, PRIMARY, agent, julia, plan_task, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::AgentCommand;
use crate::domain::ports::StepAgent;
use crate::domain::session_agent::SessionAgentError;

fn planner() -> Role {
    Role::try_from("planner".to_string()).expect("valid")
}

fn check(key: factory_records::domain::models::StepKey) -> AgentCommand {
    AgentCommand {
        key,
        owner: julia(),
        role: planner(),
        command: SessionCommand {
            cwd: ".".to_string(),
            command: "just check".to_string(),
            timeout_secs: 600,
        },
    }
}

/// A check runs in the session the Step's work ran in, as its owner and
/// through its bot, under an id of its own, and its end comes back.
#[tokio::test]
async fn a_check_runs_in_the_step_s_session() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let ended = SessionCommandOutput {
        exit_code: 1,
        stdout: "1 failed".to_string(),
        stderr: String::new(),
    };
    sessions.command_output = Some(ended.clone());
    let task = plan_task(&repo).await;
    let key = task.key;
    let agent = agent(&sessions, &repo);
    agent.answer(task).await.expect("answer");
    let claimed = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");

    let output = agent.run_check(check(key)).await.expect("run");

    assert_eq!(output, Some(ended));
    let calls = sessions.exec_calls.lock().expect("execs").clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].action.session_id.as_uuid(), claimed.session_id);
    assert_eq!(calls[0].action.owner, julia());
    assert_eq!(calls[0].action.bot_id, PRIMARY);
    assert_ne!(calls[0].action.action_id.as_uuid(), claimed.action_id);
    assert_eq!(calls[0].command, check(key).command);
}

/// A runtime that runs no command leaves the check to the agent.
#[tokio::test]
async fn a_runtime_that_runs_no_command_runs_no_check() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let key = task.key;
    let agent = agent(&sessions, &repo);
    agent.answer(task).await.expect("answer");

    let output = agent.run_check(check(key)).await.expect("run");

    assert_eq!(output, None);
}

/// A Step that let go of its session runs nothing in it.
#[tokio::test]
async fn a_step_that_let_go_of_its_session_runs_no_check() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions.command_output = Some(SessionCommandOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    });
    let task = plan_task(&repo).await;
    let key = task.key;
    let agent = agent(&sessions, &repo);
    agent.answer(task).await.expect("answer");
    repo.release_session(key).await.expect("release");

    let refused = agent.run_check(check(key)).await.expect_err("released");

    assert!(
        matches!(refused, SessionAgentError::Released),
        "{refused:?}"
    );
    assert!(sessions.exec_calls.lock().expect("execs").is_empty());
}
