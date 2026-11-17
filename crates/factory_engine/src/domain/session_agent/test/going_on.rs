use agent_session::domain::routines::RoutineActionStatus;
use factory_records::domain::models::{EntryId, HandBack, Outcome, StepKey};
use factory_records::domain::ports::RunRepository;

use super::{FakeSessions, agent, plan_task, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::AgentTask;
use crate::domain::ports::{Retryable, StepAgent};
use crate::domain::session_agent::SessionAgentError;

/// The agent's question, handed back for the Step's first prompt.
fn the_question() -> HandBack {
    HandBack {
        outcome: Outcome::question(),
        output: "CSV or JSON?".to_string(),
    }
}

/// A `plan` Step whose agent asked a question, ended, and the Step 2 that
/// goes on from it, started.
async fn asked(repo: &FakeRepo, sessions: &FakeSessions) -> (AgentTask, AgentTask) {
    let task = AgentTask {
        outcomes: vec![Outcome::done(), Outcome::question()],
        ..plan_task(repo).await
    };
    let answer = agent(sessions, repo)
        .answer(task.clone())
        .await
        .expect("answer");
    assert_eq!(answer.outcome, Outcome::question());
    repo.end_step(task.key, &answer.outcome, &answer.text)
        .await
        .expect("end");
    let going_on = StepKey {
        run_id: task.key.run_id,
        seq: 2,
    };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(going_on, &entry, 1).await.expect("start");
    let next = AgentTask {
        key: going_on,
        prompt: "The owner answered: CSV".to_string(),
        resumes: Some(task.key.seq),
        ..task.clone()
    };
    (task, next)
}

/// A Step that goes on from its agent's question opens no session: it
/// prompts the asking Step's, under a prompt of its own, and its answer is
/// what the agent handed back for that prompt. A retry asks under the same
/// prompt, which the session refuses, and reads the same answer. A stop of
/// the Step then watches that prompt, in the session it opened before.
#[tokio::test]
async fn a_step_going_on_from_a_question_prompts_the_asking_step_s_session() {
    let repo = FakeRepo::default();
    let mut sessions =
        FakeSessions::new(&repo, Some(the_question()), RoutineActionStatus::Succeeded);
    sessions.follow_up_handed = Some(the_plan());
    let (task, next) = asked(&repo, &sessions).await;
    let asking = repo
        .claimed_session(task.key)
        .await
        .expect("read")
        .expect("claimed");
    let agent = agent(&sessions, &repo);

    let answer = agent.answer(next.clone()).await.expect("go on");
    let again = agent.answer(next.clone()).await.expect("again");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(answer.text, the_plan().output);
    assert_eq!(again, answer);
    assert_eq!(sessions.prepares(), 1);
    let calls = sessions.follow_up_calls.lock().expect("follow-ups").clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].action.session_id.as_uuid(), asking.session_id);
    assert_eq!(calls[1].action.action_id, calls[0].action.action_id);
    assert_ne!(calls[0].action.action_id.as_uuid(), asking.action_id);
    assert!(calls[0].prompt.starts_with("The owner answered: CSV"));
    assert!(calls[0].prompt.contains("`done`, `question`"));
    let stopping = repo
        .stopping_session(next.key, std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    assert_eq!(stopping.session.session_id, asking.session_id);
    assert_eq!(
        stopping.session.action_id,
        calls[0].action.action_id.as_uuid()
    );
    assert!(stopping.opened);
}

/// A Step whose asking Step holds no session (a Step no agent did) has
/// nothing to go on in, and fails for good.
#[tokio::test]
async fn a_step_going_on_from_a_step_with_no_session_fails_for_good() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let next = AgentTask {
        resumes: Some(task.key.seq),
        ..task
    };

    let failed = agent(&sessions, &repo).answer(next).await;

    let Err(error) = failed else {
        panic!("there is no session to go on in");
    };
    assert!(matches!(error, SessionAgentError::NoSession));
    assert!(!error.retryable());
    assert_eq!(sessions.prepares(), 0);
}
