use agent_session::domain::routines::RoutineActionStatus;
use factory_records::domain::models::{HandBack, Outcome};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use super::{FakeSessions, agent, julia, plan_task, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::AgentTurn;
use crate::domain::ports::StepAgent;
use crate::domain::session_agent::SessionAgentError;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

/// A later prompt goes to the session the Step's work ran in, under its
/// own id, and its answer is what the agent handed back for it, not the
/// work's. A retry asks under the same id, which the session refuses, and
/// still reads that answer. A stop then watches this prompt.
#[tokio::test]
async fn a_later_prompt_goes_on_in_the_step_s_session() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    sessions.follow_up_handed = Some(HandBack {
        outcome: outcome("passed"),
        output: "web: exit 0".to_string(),
    });
    let task = plan_task(&repo).await;
    let key = task.key;
    let agent = agent(&sessions, &repo);
    agent.answer(task).await.expect("answer");
    let claimed = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");
    let turn = AgentTurn {
        key,
        owner: julia(),
        action_id: Uuid::now_v7(),
        prompt: "Run the checks".to_string(),
        outcomes: vec![outcome("passed"), outcome("failed")],
    };

    let answer = agent.follow_up(turn.clone()).await.expect("follow up");
    let again = agent.follow_up(turn.clone()).await.expect("again");

    assert_eq!(answer.outcome, outcome("passed"));
    assert_eq!(answer.text, "web: exit 0");
    assert_eq!(again, answer);
    let calls = sessions.follow_up_calls.lock().expect("follow-ups").clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].action.session_id.as_uuid(), claimed.session_id);
    assert_eq!(calls[0].action.action_id.as_uuid(), turn.action_id);
    assert!(calls[0].prompt.starts_with("Run the checks"));
    assert!(calls[0].prompt.contains("`passed`, `failed`"));
    let stopping = repo
        .stopping_session(key, std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");
    assert_eq!(stopping.session.action_id, turn.action_id);
}

/// The work's hand back is not a later prompt's: one that hands nothing
/// back fails, and a Step with no session has nothing to go on in.
#[tokio::test]
async fn a_later_prompt_needs_its_own_hand_back_and_a_session() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let key = task.key;
    let agent = agent(&sessions, &repo);
    let turn = AgentTurn {
        key,
        owner: julia(),
        action_id: Uuid::now_v7(),
        prompt: "Run the checks".to_string(),
        outcomes: vec![outcome("passed"), outcome("failed")],
    };

    let before_work = agent.follow_up(turn.clone()).await;
    agent.answer(task).await.expect("answer");
    let silent = agent.follow_up(turn).await;

    assert!(matches!(before_work, Err(SessionAgentError::NoSession)));
    assert!(matches!(silent, Err(SessionAgentError::NothingHandedBack)));
}
