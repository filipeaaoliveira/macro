use agent_session::domain::routines::RoutineActionStatus;
use factory_records::domain::models::{HandBack, Outcome};
use factory_records::domain::ports::RunRepository;

use super::{FakeSessions, agent, plan_task, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::ports::{AgentFailure, Retryable, StepAgent};
use crate::domain::session_agent::SessionAgentError;
use crate::domain::session_agent::reminder::SAID_NOTHING;

/// A turn that ends with nothing handed back is followed by one reminder,
/// in the same session under an id of its own, telling the agent to hand
/// back; what it hands back then is the Step's answer.
#[tokio::test]
async fn an_agent_that_ends_its_turn_without_a_result_is_reminded_once() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded);
    sessions.follow_up_handed = Some(the_plan());
    let task = plan_task(&repo).await;
    let key = task.key;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(answer.text, the_plan().output);
    let claimed = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");
    let reminders = sessions.follow_up_calls.lock().expect("follow-ups").clone();
    assert_eq!(reminders.len(), 1);
    let reminder = &reminders[0];
    assert_eq!(reminder.action.session_id.as_uuid(), claimed.session_id);
    assert_ne!(reminder.action.action_id.as_uuid(), claimed.action_id);
    assert_eq!(reminder.action.action_id.as_uuid().get_version_num(), 7);
    assert!(
        reminder
            .prompt
            .starts_with("## Your turn ended without a result")
    );
    assert!(reminder.prompt.contains("macro.factory.handBack"));
}

/// A turn that hands back its result is never reminded.
#[tokio::test]
async fn an_agent_that_hands_back_is_not_reminded() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    agent(&sessions, &repo)
        .answer(plan_task(&repo).await)
        .await
        .expect("answer");

    assert!(
        sessions
            .follow_up_calls
            .lock()
            .expect("follow-ups")
            .is_empty()
    );
}

/// A reminder whose turn ends with nothing handed back too is the agent
/// stopping without a result: the error carries its last message, for the
/// Run's owner, and is not retried. A retry anyway reminds under the same
/// id, which the session refuses as one it holds, and ends the same way.
#[tokio::test]
async fn an_agent_that_stays_silent_once_reminded_stops_with_its_last_message() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded);
    sessions.last_words = Some("I could not find the settings page.".to_string());
    let task = plan_task(&repo).await;
    let agent = agent(&sessions, &repo);

    let failed = agent.answer(task.clone()).await;
    let again = agent.answer(task).await;

    let Err(error) = failed else {
        panic!("nothing was handed back");
    };
    assert!(matches!(
        &error,
        SessionAgentError::StoppedWithoutResult(said) if said == "I could not find the settings page."
    ));
    assert!(!error.retryable());
    assert_eq!(
        error.stopped_without_result(),
        Some("I could not find the settings page.")
    );
    assert!(matches!(
        again,
        Err(SessionAgentError::StoppedWithoutResult(_))
    ));
    let reminders = sessions.follow_up_calls.lock().expect("follow-ups").clone();
    assert_eq!(reminders.len(), 2);
    assert_eq!(reminders[0].action.action_id, reminders[1].action.action_id);
    assert_eq!(sessions.prompts(), 2);
    assert_eq!(sessions.landed(), 1);
}

/// An agent that said nothing at all is shown as such.
#[tokio::test]
async fn an_agent_that_said_nothing_stops_saying_so() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded);

    let failed = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

    assert!(matches!(
        failed,
        Err(SessionAgentError::StoppedWithoutResult(said)) if said == SAID_NOTHING
    ));
}

/// A retry of an attempt whose reminder the agent answered reads that
/// answer: the first prompt reads as gone on from, and the reminder,
/// recorded again under the same id, keeps what was handed back.
#[tokio::test]
async fn a_retry_after_the_reminder_reads_its_answer() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded);
    sessions.follow_up_handed = Some(HandBack {
        outcome: Outcome::done(),
        output: "The plan, handed back once reminded".to_string(),
    });
    let task = plan_task(&repo).await;
    let agent = agent(&sessions, &repo);

    let first = agent.answer(task.clone()).await.expect("answer");
    let retried = agent.answer(task).await.expect("again");

    assert_eq!(retried, first);
    assert_eq!(retried.text, "The plan, handed back once reminded");
}

/// An agent that said nothing once reminded is shown what it said last in
/// its first turn.
#[tokio::test]
async fn an_agent_silent_once_reminded_is_shown_with_its_first_turn_s_words() {
    let repo = FakeRepo::default();
    let mut sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded);
    sessions.first_words = Some("The settings page has no toggle yet.".to_string());

    let failed = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

    assert!(matches!(
        failed,
        Err(SessionAgentError::StoppedWithoutResult(said))
            if said == "The settings page has no toggle yet."
    ));
}
