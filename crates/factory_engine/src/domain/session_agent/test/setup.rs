use agent_session::domain::routines::{RoutineActionStatus, RoutineSessionError};
use agent_session::domain::session_command::SessionCommandOutput;

use super::{BACKUP, FakeSessions, PRIMARY, agent, no_quota, plan_task, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::AgentTask;
use crate::domain::ports::{AgentFailure, Retryable, StepAgent};
use crate::domain::session_agent::SessionAgentError;
use crate::domain::setup::RepositorySetup;

fn setup() -> RepositorySetup {
    RepositorySetup::parse(
        r#"{
            "setup": [
                { "name": "install", "run": "bun install" },
                { "name": "generate", "run": "just generate", "timeout": 300 }
            ],
            "env": { "CACHE_DIR": "/tmp/cache" }
        }"#,
    )
    .expect("parse")
}

fn ended(exit_code: i32, stderr: &str) -> Result<SessionCommandOutput, RoutineSessionError> {
    Ok(SessionCommandOutput {
        exit_code,
        stdout: String::new(),
        stderr: stderr.to_string(),
    })
}

async fn task_with_setup(repo: &FakeRepo) -> AgentTask {
    AgentTask {
        setup: Some(setup()),
        ..plan_task(repo).await
    }
}

fn commands(sessions: &FakeSessions) -> Vec<String> {
    sessions
        .exec_calls
        .lock()
        .expect("execs")
        .iter()
        .map(|call| call.command.command.clone())
        .collect()
}

/// Before the first prompt of a new session, each setup command runs in
/// it, in order, from the checkout's root, with the setup's values and
/// under its own timeout; then the agent is prompted.
#[tokio::test]
async fn a_new_session_runs_the_setup_before_its_first_prompt() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions
        .exec_replies
        .lock()
        .expect("replies")
        .extend([ended(0, ""), ended(0, "")]);
    let task = task_with_setup(&repo).await;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.text, the_plan().output);
    assert_eq!(
        commands(&sessions),
        [
            "export CACHE_DIR='/tmp/cache'\ntimeout --kill-after=10 600 bash --noprofile --norc -c 'bun install'",
            "export CACHE_DIR='/tmp/cache'\ntimeout --kill-after=10 300 bash --noprofile --norc -c 'just generate'",
        ]
    );
    let calls = sessions.exec_calls.lock().expect("execs");
    assert!(calls.iter().all(|call| call.command.cwd == "."));
    assert_eq!(calls[1].command.timeout_secs, 320);
    assert!(calls.iter().all(|call| call.action.bot_id == PRIMARY));
    assert_eq!(sessions.prompts(), 1);
    let claimed = repo.sessions.lock().expect("sessions");
    assert_eq!(claimed[0].setup, Some((claimed[0].session.session_id, 2)));
}

/// A setup command that fails ends the Step for good with the command,
/// its exit code and the end of its output, and the agent is never
/// prompted.
#[tokio::test]
async fn a_failing_setup_ends_the_step_with_its_output_and_no_prompt() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions
        .exec_replies
        .lock()
        .expect("replies")
        .extend([ended(0, ""), ended(2, "error: generate.ts not found")]);
    let task = task_with_setup(&repo).await;

    let failed = agent(&sessions, &repo).answer(task).await;

    let Err(error) = failed else {
        panic!("the setup failed");
    };
    assert!(!error.retryable());
    let SessionAgentError::SetupFailed(report) = &error else {
        panic!("{error:?}");
    };
    assert_eq!(error.setup_failed(), Some(report.as_str()));
    assert!(report.contains("`generate` (`just generate`"), "{report}");
    assert!(report.contains("exited with status 2"), "{report}");
    assert!(report.contains("error: generate.ts not found"), "{report}");
    assert_eq!(sessions.prompts(), 0);
    assert_eq!(commands(&sessions).len(), 2);
}

/// A retry in the same session skips the setup commands that passed, and
/// goes on with the rest; one after the whole setup passed runs none.
#[tokio::test]
async fn a_retry_in_the_same_session_skips_the_setup_that_passed() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions
        .exec_replies
        .lock()
        .expect("replies")
        .extend([ended(0, ""), Err(RoutineSessionError::RuntimeUnavailable)]);
    let task = task_with_setup(&repo).await;
    let agent = agent(&sessions, &repo);

    let first = agent.answer(task.clone()).await;
    let Err(error) = first else {
        panic!("the runtime went away");
    };
    assert!(error.retryable(), "{error:?}");
    assert_eq!(sessions.prompts(), 0);

    sessions
        .exec_replies
        .lock()
        .expect("replies")
        .push_back(ended(0, ""));
    agent.answer(task.clone()).await.expect("retried");
    let ran = commands(&sessions);
    assert_eq!(ran.len(), 3);
    assert!(ran[2].ends_with("'just generate'"), "{}", ran[2]);
    assert_eq!(sessions.prompts(), 1);

    agent.answer(task).await.expect("retried again");
    assert_eq!(commands(&sessions).len(), 3, "the setup passed already");
}

/// A session the Step goes on in when its Agent could not start runs the
/// setup again: it is a new checkout.
#[tokio::test]
async fn a_session_handed_to_the_next_agent_runs_the_setup_again() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .prompting(&[no_quota()]);
    sessions.command_output = Some(SessionCommandOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    });
    let task = task_with_setup(&repo).await;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    let bots: Vec<_> = sessions
        .exec_calls
        .lock()
        .expect("execs")
        .iter()
        .map(|call| call.action.bot_id)
        .collect();
    assert_eq!(bots, [PRIMARY, PRIMARY, BACKUP, BACKUP]);
}

/// Where the session's runtime runs no command, a repository with a setup
/// ends the Step for good, saying why, and the agent is never prompted.
#[tokio::test]
async fn a_session_that_runs_no_command_cannot_be_set_up() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = task_with_setup(&repo).await;

    let failed = agent(&sessions, &repo).answer(task).await;

    let Err(error) = failed else {
        panic!("the setup cannot run");
    };
    assert!(!error.retryable());
    let Some(report) = error.setup_failed() else {
        panic!("{error:?}");
    };
    assert!(report.contains(".macro/setup.json"), "{report}");
    assert!(report.contains("runs no command"), "{report}");
    assert_eq!(sessions.prompts(), 0);
}

/// A command that would run past the time the Step gives its work ends the
/// Step, saying so: a retry would have no more time.
#[tokio::test]
async fn a_setup_command_past_the_step_s_time_ends_the_step() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = AgentTask {
        answer_by: Some(chrono::Utc::now() + chrono::TimeDelta::seconds(120)),
        ..task_with_setup(&repo).await
    };

    let failed = agent(&sessions, &repo).answer(task).await;

    let Err(error) = failed else {
        panic!("no time for the setup");
    };
    assert!(!error.retryable(), "{error:?}");
    let Some(report) = error.setup_failed() else {
        panic!("{error:?}");
    };
    assert!(
        report.starts_with("The step's time ran out before setup command `install`"),
        "{report}"
    );
    assert!(commands(&sessions).is_empty());
    assert_eq!(sessions.prompts(), 0);
}
