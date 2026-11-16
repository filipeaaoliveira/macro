use std::sync::{Arc, Mutex};

use agent_session::domain::routines::{
    PrepareRoutineSession, PreparedRoutineSession, PromptRoutineSession, RoutineActionStatus,
    RoutineFailureReason, RoutinePromptAccepted, RoutineSessionAction, RoutineSessionError,
    RoutineSessions, ValidateRoutineSession, ValidatedRoutineSession,
};
use bot_id::BotId;
use factory_records::domain::models::{EntryId, HandBack, Outcome, RunId, RunInsert, StepKey};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{SessionAgent, SessionAgentError};
use crate::domain::fakes::FakeRepo;
use crate::domain::flow::StepName;
use crate::domain::models::AgentTask;
use crate::domain::ports::StepAgent;

/// Agent sessions whose agent hands `handed` back when prompted, and whose
/// turn then ends with `status`. Keeps what it was asked.
#[derive(Clone)]
struct FakeSessions {
    repo: FakeRepo,
    handed: Option<HandBack>,
    status: RoutineActionStatus,
    /// Whether the session already has its prompt, as after an attempt
    /// that prompted it.
    prompted: bool,
    prepared: Arc<Mutex<Vec<PrepareRoutineSession>>>,
    prompts: Arc<Mutex<Vec<PromptRoutineSession>>>,
    cancelled: Arc<Mutex<Vec<RoutineSessionAction>>>,
}

impl FakeSessions {
    fn new(repo: &FakeRepo, handed: Option<HandBack>, status: RoutineActionStatus) -> Self {
        Self {
            repo: repo.clone(),
            handed,
            status,
            prompted: false,
            prepared: Arc::default(),
            prompts: Arc::default(),
            cancelled: Arc::default(),
        }
    }
}

impl RoutineSessions for FakeSessions {
    async fn validate(
        &self,
        _command: ValidateRoutineSession,
    ) -> Result<ValidatedRoutineSession, RoutineSessionError> {
        Ok(ValidatedRoutineSession { managed: false })
    }

    async fn prepare(
        &self,
        command: PrepareRoutineSession,
    ) -> Result<PreparedRoutineSession, RoutineSessionError> {
        let session_id = command.session_id;
        self.prepared.lock().expect("prepared").push(command);
        Ok(PreparedRoutineSession { session_id })
    }

    async fn prompt(
        &self,
        command: PromptRoutineSession,
    ) -> Result<RoutinePromptAccepted, RoutineSessionError> {
        if self.prompted {
            return Err(RoutineSessionError::Conflict);
        }
        // The agent works and hands back from the session, as code mode
        // would through the Run API.
        if let Some(handed) = &self.handed {
            self.repo
                .hand_back(command.action.session_id.as_uuid(), handed)
                .await
                .expect("hand back");
        }
        let action_id = command.action.action_id;
        self.prompts.lock().expect("prompts").push(command);
        Ok(RoutinePromptAccepted {
            action_id,
            queued: false,
        })
    }

    async fn status(
        &self,
        _command: RoutineSessionAction,
    ) -> Result<RoutineActionStatus, RoutineSessionError> {
        Ok(self.status)
    }

    async fn cancel(&self, command: RoutineSessionAction) -> Result<(), RoutineSessionError> {
        self.cancelled.lock().expect("cancelled").push(command);
        Ok(())
    }
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

/// A `plan` task of a new Run of Julia's, its Step started.
async fn plan_task(repo: &FakeRepo) -> AgentTask {
    let run = repo
        .insert_run(RunId::generate(), "Add a dark mode toggle", &julia(), None)
        .await
        .expect("insert");
    let RunInsert::Inserted(run) = run else {
        panic!("a run with no task is kept");
    };
    let key = StepKey {
        run_id: run.id,
        seq: 1,
    };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    AgentTask {
        key,
        owner: julia(),
        step: StepName::try_from("plan".to_string()).expect("valid"),
        prompt: "Plan the change".to_string(),
        outcomes: vec![Outcome::done()],
    }
}

fn the_plan() -> HandBack {
    HandBack {
        outcome: Outcome::done(),
        output: "Add a toggle to the settings page".to_string(),
    }
}

#[tokio::test]
async fn a_step_runs_in_a_session_of_its_owner_and_returns_what_the_agent_handed_back() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let bot_id = BotId::new_from_uuid(Uuid::now_v7());
    let agent = SessionAgent::new(sessions.clone(), repo.clone(), bot_id);
    let task = plan_task(&repo).await;

    let answer = agent.answer(task).await.expect("answer");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(answer.text, "Add a toggle to the settings page");
    let prepared = sessions.prepared.lock().expect("prepared");
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].selection.owner, julia());
    assert_eq!(prepared[0].selection.bot_id, bot_id);
    let prompts = sessions.prompts.lock().expect("prompts");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].prompt.starts_with("Plan the change"));
    assert!(prompts[0].prompt.contains("macro.factory.handBack"));
}

#[tokio::test]
async fn a_retried_step_goes_on_in_the_session_it_already_prompted() {
    let repo = FakeRepo::default();
    let first = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let bot_id = BotId::new_from_uuid(Uuid::now_v7());
    let task = plan_task(&repo).await;
    SessionAgent::new(first, repo.clone(), bot_id)
        .answer(task.clone())
        .await
        .expect("first attempt");
    let retry = FakeSessions {
        prompted: true,
        ..FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded)
    };

    let answer = SessionAgent::new(retry.clone(), repo.clone(), bot_id)
        .answer(task)
        .await
        .expect("retry");

    assert_eq!(answer.text, "Add a toggle to the settings page");
    assert!(retry.prepared.lock().expect("prepared").is_empty());
    assert!(retry.prompts.lock().expect("prompts").is_empty());
}

#[tokio::test]
async fn a_turn_that_ends_with_nothing_handed_back_fails_the_step() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded);
    let agent = SessionAgent::new(sessions, repo.clone(), BotId::new_from_uuid(Uuid::now_v7()));

    let failed = agent.answer(plan_task(&repo).await).await;

    assert!(matches!(failed, Err(SessionAgentError::NothingHandedBack)));
}

#[tokio::test]
async fn a_turn_that_fails_fails_the_step() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(
        &repo,
        Some(the_plan()),
        RoutineActionStatus::Failed(RoutineFailureReason::Refusal),
    );
    let agent = SessionAgent::new(sessions, repo.clone(), BotId::new_from_uuid(Uuid::now_v7()));

    let failed = agent.answer(plan_task(&repo).await).await;

    assert!(matches!(
        failed,
        Err(SessionAgentError::TurnFailed(RoutineFailureReason::Refusal))
    ));
}

#[tokio::test]
async fn stopping_a_step_cancels_the_session_it_claimed() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let agent = SessionAgent::new(
        sessions.clone(),
        repo.clone(),
        BotId::new_from_uuid(Uuid::now_v7()),
    );
    let task = plan_task(&repo).await;
    let key = task.key;
    agent.answer(task).await.expect("answer");

    agent.stop(key, &julia()).await.expect("stop");

    let claimed = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");
    let cancelled = sessions.cancelled.lock().expect("cancelled").clone();
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].session_id.as_uuid(), claimed.session_id);
}
