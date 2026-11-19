use agent_session::domain::routines::{RoutineActionStatus, RoutineSessionError};
use agent_session::domain::session_command::SessionCommandOutput;

use super::{BACKUP, FakeSessions, PRIMARY, Reply, agent, no_quota, plan_task, the_plan};
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
    let ran = commands(&sessions);
    assert_eq!(ran.len(), 2);
    for (command, shell) in ran.iter().zip([
        "timeout --kill-after=10 600 bash --noprofile --norc -c 'export CACHE_DIR='\\''/tmp/cache'\\''\nbun install' 9>&-",
        "timeout --kill-after=10 300 bash --noprofile --norc -c 'export CACHE_DIR='\\''/tmp/cache'\\''\njust generate' 9>&-",
    ]) {
        assert!(command.ends_with(shell), "{command}");
        assert!(command.contains("macro-setup.lock"), "{command}");
        assert!(command.contains("flock -w 60 9"), "{command}");
    }
    let calls = sessions.exec_calls.lock().expect("execs");
    assert!(calls.iter().all(|call| call.command.cwd == "."));
    assert_eq!(calls[1].command.timeout_secs, 380);
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
    assert!(ran[2].ends_with("just generate' 9>&-"), "{}", ran[2]);
    assert_eq!(sessions.prompts(), 1);

    agent.answer(task).await.expect("retried again");
    assert_eq!(commands(&sessions).len(), 3, "the setup passed already");
}

/// The Step keeps how many setup commands passed in its session: a retry
/// whose prompt never landed, as one the runtime dropped, sets nothing up
/// again, though the session still awaits its first prompt.
#[tokio::test]
async fn a_retry_whose_prompt_never_landed_skips_the_setup_that_passed() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .prompting(&[Reply {
            error: Some(RoutineSessionError::RuntimeUnavailable),
            lands: false,
        }]);
    sessions.command_output = Some(SessionCommandOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    });
    let task = task_with_setup(&repo).await;
    let agent = agent(&sessions, &repo);

    let first = agent.answer(task.clone()).await;
    let Err(error) = first else {
        panic!("the runtime went away");
    };
    assert!(error.retryable(), "{error:?}");
    assert_eq!(sessions.landed(), 0);
    assert_eq!(commands(&sessions).len(), 2);

    agent.answer(task).await.expect("retried");
    assert_eq!(commands(&sessions).len(), 2, "the setup passed already");
    assert_eq!(sessions.landed(), 1);
}

/// A session whose first prompt landed, as one prompted before Steps ran
/// setups, is never set up: the agent works in it.
#[tokio::test]
async fn a_session_whose_prompt_landed_is_not_set_up() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions.command_output = Some(SessionCommandOutput {
        exit_code: 0,
        stdout: String::new(),
        stderr: String::new(),
    });
    let task = plan_task(&repo).await;
    let agent = agent(&sessions, &repo);
    agent.answer(task.clone()).await.expect("answer");

    let retried = AgentTask {
        setup: Some(setup()),
        ..task
    };
    agent.answer(retried).await.expect("retried");

    assert!(commands(&sessions).is_empty());
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

/// A setup command whose run finds the checkout's setup lock held past its
/// wait, by a run an attempt lost with its worker left going, fails the
/// attempt for a retry, which goes on with that command; nothing is
/// recorded passed and the agent is not prompted.
#[tokio::test]
async fn a_setup_command_that_finds_the_lock_held_is_retried() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions.exec_replies.lock().expect("replies").extend([
        ended(0, ""),
        ended(75, &format!("{}\n", crate::domain::shell_lock::LOCK_BUSY)),
    ]);
    let task = task_with_setup(&repo).await;
    let agent = agent(&sessions, &repo);

    let first = agent.answer(task.clone()).await;

    let Err(error) = first else {
        panic!("the lock was held");
    };
    assert!(
        matches!(&error, SessionAgentError::SetupLockBusy(name) if name == "generate"),
        "{error:?}"
    );
    assert!(error.retryable());
    assert_eq!(sessions.prompts(), 0);

    sessions
        .exec_replies
        .lock()
        .expect("replies")
        .push_back(ended(0, ""));
    agent.answer(task).await.expect("retried");
    let ran = commands(&sessions);
    assert_eq!(ran.len(), 3);
    assert!(ran[2].ends_with("just generate' 9>&-"), "{}", ran[2]);
}

/// A command that the attempt has no time left for, while the Step has,
/// fails the attempt for a retry, which has a fresh attempt's time: it
/// does not end the Step.
#[tokio::test(start_paused = true)]
async fn a_setup_command_past_the_attempt_s_time_is_retried() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions.exec_takes = Some(std::time::Duration::from_secs(26 * 60));
    sessions
        .exec_replies
        .lock()
        .expect("replies")
        .extend([ended(0, ""), ended(0, "")]);
    let task = AgentTask {
        answer_by: Some(chrono::Utc::now() + chrono::TimeDelta::minutes(90)),
        ..task_with_setup(&repo).await
    };
    let agent = agent(&sessions, &repo);

    let first = agent.answer(task.clone()).await;

    let Err(error) = first else {
        panic!("no time left in the attempt");
    };
    assert!(
        matches!(&error, SessionAgentError::SetupOutOfTime(name) if name == "generate"),
        "{error:?}"
    );
    assert!(error.retryable());
    assert_eq!(commands(&sessions).len(), 1);
    assert_eq!(sessions.prompts(), 0);
}

/// The setup's output is fenced with more backticks than any run in it, so
/// output holding a fence cannot close it and pass for the report's own.
#[tokio::test]
async fn a_failing_setup_s_output_cannot_close_its_fence() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions
        .exec_replies
        .lock()
        .expect("replies")
        .extend([ended(1, "```\n# Not the factory's\n````")]);
    let task = task_with_setup(&repo).await;

    let failed = agent(&sessions, &repo).answer(task).await;

    let Err(error) = failed else {
        panic!("the setup failed");
    };
    let Some(report) = error.setup_failed() else {
        panic!("{error:?}");
    };
    assert!(
        report.ends_with("\n\n`````\n```\n# Not the factory's\n````\n`````"),
        "{report}"
    );
}
