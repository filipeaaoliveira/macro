use agent_session::domain::routines::RoutineActionStatus;
use bot_id::BotId;
use factory_records::domain::models::PassedOver;
use factory_records::domain::ports::RunRepository;

use super::{
    BACKUP, FakeSessions, PRIMARY, agents, name, no_quota, plan_task, planner, recorded, the_plan,
};
use crate::domain::fakes::FakeRepo;
use crate::domain::ports::StepAgent;
use crate::domain::session_agent::SessionAgent;
use crate::domain::session_agent::quota_memory::{NO_QUOTA_LATELY, QUOTA_COOL_DOWN};

/// The agent over `sessions` that shares what `earlier` remembers of
/// refusals, as one worker's agent does from Step to Step.
fn sharing(
    sessions: &FakeSessions,
    repo: &FakeRepo,
    earlier: &SessionAgent<FakeSessions, FakeRepo>,
) -> SessionAgent<FakeSessions, FakeRepo> {
    SessionAgent {
        sessions: sessions.clone(),
        repo: repo.clone(),
        agents: agents(),
        quota: earlier.quota.clone(),
    }
}

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

/// An Agent refused for lack of quota is skipped by the owner's next Step:
/// no session of it is opened, the Step records it passed over as refused
/// lately, and the next Agent of the Role does the Step.
#[tokio::test(start_paused = true)]
async fn an_agent_refused_lately_is_skipped_without_a_session() {
    let repo = FakeRepo::default();
    let refusing = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let first = SessionAgent::new(refusing.clone(), repo.clone(), agents());
    first
        .answer(plan_task(&repo).await)
        .await
        .expect("the first step");
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let key = task.key;

    let answer = sharing(&sessions, &repo, &first)
        .answer(task)
        .await
        .expect("answer");

    assert_eq!(answer.text, the_plan().output);
    assert_eq!(prepared_bots(&sessions), [BACKUP]);
    let (_, agent, passed) = recorded(&repo, key);
    assert_eq!(agent, Some(name("backup")));
    assert_eq!(
        passed,
        [PassedOver {
            agent: name("primary"),
            reason: NO_QUOTA_LATELY.to_string(),
        }]
    );
}

/// Once the cool-down passed, the Agent is asked again: recovery is found
/// by asking. One that starts is forgotten, so the Step after it asks it
/// too.
#[tokio::test(start_paused = true)]
async fn an_agent_is_asked_again_once_the_cool_down_passed() {
    let repo = FakeRepo::default();
    let refusing = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let first = SessionAgent::new(refusing, repo.clone(), agents());
    first
        .answer(plan_task(&repo).await)
        .await
        .expect("the first step");
    tokio::time::advance(QUOTA_COOL_DOWN).await;
    let recovered = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let after = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    sharing(&recovered, &repo, &first)
        .answer(plan_task(&repo).await)
        .await
        .expect("the recovered step");
    sharing(&after, &repo, &first)
        .answer(plan_task(&repo).await)
        .await
        .expect("the step after");

    assert_eq!(prepared_bots(&recovered), [PRIMARY]);
    assert_eq!(prepared_bots(&after), [PRIMARY]);
}

/// The Role's last Agent is never skipped, however lately it was refused:
/// the Step asks it, so a quota that recovered is found.
#[tokio::test(start_paused = true)]
async fn the_role_s_last_agent_is_always_asked() {
    let repo = FakeRepo::default();
    let refusing = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota(), no_quota()]);
    let first = SessionAgent::new(refusing, repo.clone(), agents());
    first
        .answer(plan_task(&repo).await)
        .await
        .expect_err("every agent refused");
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);

    sharing(&sessions, &repo, &first)
        .answer(plan_task(&repo).await)
        .await
        .expect("the last agent recovered");

    assert_eq!(prepared_bots(&sessions), [BACKUP]);
}

/// A retried Step whose session an earlier attempt claimed is never
/// skipped: that attempt may have started work in it.
#[tokio::test(start_paused = true)]
async fn a_retried_step_goes_on_with_its_agent_however_lately_refused() {
    let repo = FakeRepo::default();
    let refusing = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let first = SessionAgent::new(refusing, repo.clone(), agents());
    first
        .answer(plan_task(&repo).await)
        .await
        .expect("the first step");
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let earlier = SessionAgent::new(sessions.clone(), repo.clone(), agents());
    repo.claim_session(
        task.key,
        earlier.new_session(&name("primary")).expect("a session"),
        &planner(),
        &task.outcomes,
    )
    .await
    .expect("an earlier attempt's claim");

    sharing(&sessions, &repo, &first)
        .answer(task)
        .await
        .expect("answer");

    assert_eq!(prepared_bots(&sessions), [PRIMARY]);
}
