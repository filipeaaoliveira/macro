//! A turn the usage limit of its runtime's own provider account ends, as a
//! Codex or Claude Code subscription signed in on a machine running macrod
//! does: the Agent is out of quota, not the work failed.

use agent_session::domain::routines::{RoutineActionStatus, RoutineFailureReason};
use bot_id::BotId;
use factory_records::domain::models::PassedOver;

use super::{BACKUP, FakeSessions, PRIMARY, agents, name, plan_task, recorded, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::ports::{AgentFailure, Retryable, StepAgent};
use crate::domain::session_agent::quota_memory::NO_QUOTA_LATELY;
use crate::domain::session_agent::{SessionAgent, SessionAgentError, USAGE_LIMIT};

/// The bots of the sessions `sessions` was asked to open, in order.
fn prepared_bots(sessions: &FakeSessions) -> Vec<BotId> {
    sessions
        .prepared_calls
        .lock()
        .expect("prepared")
        .iter()
        .map(|call| call.selection.bot_id)
        .collect()
}

fn usage_limit() -> SessionAgentError {
    SessionAgentError::TurnFailed(RoutineFailureReason::UsageLimit)
}

/// A first turn its usage limit ended before the agent called any tool
/// changed nothing: the next Agent of the Role takes the Step in a session
/// of its own, the first one deleted, and the Step records the Agent passed
/// over for its usage limit.
#[tokio::test(start_paused = true)]
async fn a_usage_limit_before_any_tool_hands_the_step_to_the_next_agent() {
    let repo = FakeRepo::default();
    let sessions =
        FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded).limiting(1);
    let task = plan_task(&repo).await;
    let key = task.key;

    let answer = SessionAgent::new(sessions.clone(), repo.clone(), agents())
        .answer(task)
        .await
        .expect("the backup does the step");

    assert_eq!(answer.text, the_plan().output);
    assert_eq!(prepared_bots(&sessions), [PRIMARY, BACKUP]);
    let deleted = sessions.deleted.lock().expect("deleted");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].bot_id, PRIMARY);
    let (_, agent, passed) = recorded(&repo, key);
    assert_eq!(agent, Some(name("backup")));
    assert_eq!(
        passed,
        [PassedOver {
            agent: name("primary"),
            reason: USAGE_LIMIT.to_string(),
        }]
    );
}

/// A turn that called tools before its usage limit did work, which stays in
/// its session: no other Agent starts it over. The Step ends out of quota,
/// for good, so its Run reads as stopped for infrastructure, not failed.
#[tokio::test(start_paused = true)]
async fn a_usage_limit_after_work_ends_the_step_out_of_quota() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions {
        tool_calls: 3,
        ..FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded).limiting(1)
    };
    let task = plan_task(&repo).await;
    let key = task.key;

    let error = SessionAgent::new(sessions.clone(), repo.clone(), agents())
        .answer(task)
        .await
        .expect_err("the usage limit ends the step");

    assert!(matches!(
        &error,
        SessionAgentError::TurnFailed(RoutineFailureReason::UsageLimit)
    ));
    assert!(error.out_of_quota());
    assert!(!error.retryable());
    assert_eq!(prepared_bots(&sessions), [PRIMARY]);
    assert_eq!(*sessions.activity_reads.lock().expect("reads"), 1);
    let (_, agent, passed) = recorded(&repo, key);
    assert_eq!(agent, Some(name("primary")));
    assert!(passed.is_empty());
}

/// When the Role's last Agent reaches its usage limit too, the Step ends
/// out of quota, the Agents before it recorded passed over.
#[tokio::test(start_paused = true)]
async fn the_role_s_last_agent_at_its_usage_limit_ends_the_step_out_of_quota() {
    let repo = FakeRepo::default();
    let sessions =
        FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded).limiting(2);
    let task = plan_task(&repo).await;
    let key = task.key;

    let error = SessionAgent::new(sessions.clone(), repo.clone(), agents())
        .answer(task)
        .await
        .expect_err("every agent is at its usage limit");

    assert!(error.out_of_quota());
    assert_eq!(prepared_bots(&sessions), [PRIMARY, BACKUP]);
    let (_, agent, passed) = recorded(&repo, key);
    assert_eq!(agent, Some(name("backup")));
    assert_eq!(
        passed,
        [PassedOver {
            agent: name("primary"),
            reason: USAGE_LIMIT.to_string(),
        }]
    );
}

/// An Agent at its usage limit is remembered as one its quota refused: the
/// owner's next Step starts with the next Agent, without a session of it.
#[tokio::test(start_paused = true)]
async fn an_agent_at_its_usage_limit_is_skipped_by_the_next_step() {
    let repo = FakeRepo::default();
    let limited = FakeSessions {
        tool_calls: 1,
        ..FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded).limiting(1)
    };
    let first = SessionAgent::new(limited, repo.clone(), agents());
    let error = first
        .answer(plan_task(&repo).await)
        .await
        .expect_err("the first step ran into the usage limit");
    assert!(matches!(
        &error,
        SessionAgentError::TurnFailed(RoutineFailureReason::UsageLimit)
    ));
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let next = SessionAgent {
        sessions: sessions.clone(),
        repo: repo.clone(),
        agents: agents(),
        quota: first.quota.clone(),
        pause_notices: None,
    };
    let task = plan_task(&repo).await;
    let key = task.key;

    next.answer(task).await.expect("the backup does the step");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
    let (_, _, passed) = recorded(&repo, key);
    assert_eq!(
        passed,
        [PassedOver {
            agent: name("primary"),
            reason: NO_QUOTA_LATELY.to_string(),
        }]
    );
}

/// Any other failed turn is the work's, as before: not out of quota, and
/// no other Agent takes the Step.
#[tokio::test(start_paused = true)]
async fn another_failed_turn_is_not_out_of_quota() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(
        &repo,
        None,
        RoutineActionStatus::Failed(RoutineFailureReason::RuntimeError),
    );

    let error = SessionAgent::new(sessions.clone(), repo.clone(), agents())
        .answer(plan_task(&repo).await)
        .await
        .expect_err("the turn failed");

    assert!(!error.out_of_quota());
    assert!(!matches!(error, SessionAgentError::NoQuota(_)));
    assert_ne!(error.to_string(), usage_limit().to_string());
    assert_eq!(prepared_bots(&sessions), [PRIMARY]);
}
