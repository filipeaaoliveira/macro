use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agent_session::domain::model::SessionVisibility;
use agent_session::domain::routines::{
    PrepareRoutineSession, PreparedRoutineSession, PromptRoutineSession, RoutineActionStatus,
    RoutineFailureReason, RoutinePendingReason, RoutinePromptAccepted, RoutineSessionAction,
    RoutineSessionError, RoutineSessions, ValidateRoutineSession, ValidatedRoutineSession,
};
use bot_id::BotId;
use factory_records::domain::models::{
    EntryId, HandBack, Outcome, RunId, RunInsert, StepKey, StepSession,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{SessionAgent, SessionAgentError};
use crate::domain::fakes::FakeRepo;
use crate::domain::flow::StepName;
use crate::domain::models::AgentTask;
use crate::domain::ports::{Retryable, StepAgent};

/// What happens to one call: it works, or fails with this error. A call
/// that fails ambiguously may still take effect (`lands`).
#[derive(Clone, Copy)]
struct Reply {
    error: Option<RoutineSessionError>,
    lands: bool,
}

const WORKS: Reply = Reply {
    error: None,
    lands: true,
};

/// The agent sessions of one Step. The session is open once a prepare
/// lands; the prompt is in its history once a prompt lands, and the agent
/// then hands `handed` back. The turn ends with `ending`; until the prompt
/// lands the status says `AwaitingPrompt`, or `Disconnected` for a session
/// whose runtime went away. Each call takes its next reply, else works. A
/// deleted session is gone: a later prepare opens a new one under its id.
#[derive(Clone)]
struct FakeSessions {
    repo: FakeRepo,
    handed: Option<HandBack>,
    ending: RoutineActionStatus,
    prepares: Arc<Mutex<VecDeque<Reply>>>,
    prompts: Arc<Mutex<VecDeque<Reply>>>,
    open: Arc<Mutex<bool>>,
    prompted: Arc<Mutex<bool>>,
    prepared_calls: Arc<Mutex<Vec<PrepareRoutineSession>>>,
    prompt_calls: Arc<Mutex<Vec<PromptRoutineSession>>>,
    /// Prompts that landed in a session's history.
    landed: Arc<Mutex<usize>>,
    /// Reads of the status that fail before one works.
    failing_reads: Arc<Mutex<usize>>,
    disconnected: bool,
    /// The Step lets go of its session while a prepare is in flight, as
    /// when Temporal gave the attempt up and another one ended the Step.
    releases_on_prepare: bool,
    cancelled: Arc<Mutex<Vec<RoutineSessionAction>>>,
    deleted: Arc<Mutex<Vec<RoutineSessionAction>>>,
    /// Stops the session ignores before its turn ends cancelled, as when a
    /// prompt on its way lands after the first stop.
    ignored_stops: usize,
}

impl FakeSessions {
    fn new(repo: &FakeRepo, handed: Option<HandBack>, ending: RoutineActionStatus) -> Self {
        Self {
            repo: repo.clone(),
            handed,
            ending,
            prepares: Arc::default(),
            prompts: Arc::default(),
            open: Arc::default(),
            prompted: Arc::default(),
            prepared_calls: Arc::default(),
            prompt_calls: Arc::default(),
            landed: Arc::default(),
            failing_reads: Arc::default(),
            disconnected: false,
            releases_on_prepare: false,
            cancelled: Arc::default(),
            deleted: Arc::default(),
            ignored_stops: 0,
        }
    }

    fn preparing(self, replies: &[Reply]) -> Self {
        self.prepares.lock().expect("prepares").extend(replies);
        self
    }

    fn prompting(self, replies: &[Reply]) -> Self {
        self.prompts.lock().expect("prompts").extend(replies);
        self
    }

    fn prepares(&self) -> usize {
        self.prepared_calls.lock().expect("prepared").len()
    }

    fn prompts(&self) -> usize {
        self.prompt_calls.lock().expect("prompts").len()
    }

    fn landed(&self) -> usize {
        *self.landed.lock().expect("landed")
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
        self.prepared_calls.lock().expect("prepared").push(command);
        if self.releases_on_prepare {
            let key = self
                .repo
                .sessions
                .lock()
                .expect("sessions")
                .iter()
                .find(|claimed| claimed.session.session_id == session_id.as_uuid())
                .map(|claimed| claimed.key)
                .expect("a claimed session");
            self.repo.release_session(key).await.expect("release");
        }
        let reply = self
            .prepares
            .lock()
            .expect("prepares")
            .pop_front()
            .unwrap_or(WORKS);
        let mut open = self.open.lock().expect("open");
        let was_open = *open;
        if reply.lands {
            *open = true;
        }
        match reply.error {
            Some(error) => Err(error),
            None if was_open => Err(RoutineSessionError::Conflict),
            None => Ok(PreparedRoutineSession { session_id }),
        }
    }

    async fn prompt(
        &self,
        command: PromptRoutineSession,
    ) -> Result<RoutinePromptAccepted, RoutineSessionError> {
        let action_id = command.action.action_id;
        let session_id = command.action.session_id.as_uuid();
        self.prompt_calls.lock().expect("prompts").push(command);
        if *self.prompted.lock().expect("prompted") {
            return Err(RoutineSessionError::Conflict);
        }
        let reply = self
            .prompts
            .lock()
            .expect("prompts")
            .pop_front()
            .unwrap_or(WORKS);
        if reply.lands {
            *self.prompted.lock().expect("prompted") = true;
            *self.landed.lock().expect("landed") += 1;
            // The agent works and hands back from the session, as code mode
            // would through the Run API.
            if let Some(handed) = &self.handed {
                self.repo
                    .hand_back(session_id, handed)
                    .await
                    .expect("hand back");
            }
        }
        match reply.error {
            Some(error) => Err(error),
            None => Ok(RoutinePromptAccepted {
                action_id,
                queued: false,
            }),
        }
    }

    async fn status(
        &self,
        _command: RoutineSessionAction,
    ) -> Result<RoutineActionStatus, RoutineSessionError> {
        {
            let mut failing = self.failing_reads.lock().expect("failing reads");
            if *failing > 0 {
                *failing -= 1;
                return Err(RoutineSessionError::OperationFailed);
            }
        }
        if !*self.open.lock().expect("open") {
            return Err(RoutineSessionError::OperationFailed);
        }
        let stops = self.cancelled.lock().expect("cancelled").len();
        if stops > self.ignored_stops {
            Ok(RoutineActionStatus::Failed(RoutineFailureReason::Cancelled))
        } else if *self.prompted.lock().expect("prompted") {
            Ok(self.ending)
        } else if self.disconnected {
            Ok(RoutineActionStatus::Pending(
                RoutinePendingReason::Disconnected,
            ))
        } else {
            Ok(RoutineActionStatus::Pending(
                RoutinePendingReason::AwaitingPrompt,
            ))
        }
    }

    async fn cancel(&self, command: RoutineSessionAction) -> Result<(), RoutineSessionError> {
        if !*self.open.lock().expect("open") {
            return Err(RoutineSessionError::OperationFailed);
        }
        self.cancelled.lock().expect("cancelled").push(command);
        Ok(())
    }

    async fn delete(&self, command: RoutineSessionAction) -> Result<(), RoutineSessionError> {
        self.deleted.lock().expect("deleted").push(command);
        let mut open = self.open.lock().expect("open");
        if !*open {
            return Err(RoutineSessionError::OperationFailed);
        }
        *open = false;
        *self.prompted.lock().expect("prompted") = false;
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

fn agent(sessions: &FakeSessions, repo: &FakeRepo) -> SessionAgent<FakeSessions, FakeRepo> {
    SessionAgent::new(
        sessions.clone(),
        repo.clone(),
        BotId::new_from_uuid(Uuid::nil()),
    )
}

#[tokio::test]
async fn a_step_runs_in_a_session_of_its_owner_and_returns_what_the_agent_handed_back() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(answer.text, "Add a toggle to the settings page");
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].selection.owner, julia());
    assert_eq!(prepared[0].visibility, SessionVisibility::Hidden);
    assert!(sessions.deleted.lock().expect("deleted").is_empty());
    let prompts = sessions.prompt_calls.lock().expect("prompts");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].prompt.starts_with("Plan the change"));
    assert!(prompts[0].prompt.contains("macro.factory.handBack"));
}

/// A retry whose first attempt opened and prompted the session finds both
/// done: the session's conflicts on prepare and prompt say so, so nothing is
/// sent again. That holds for a retry after an attempt whose answer Temporal
/// lost: the session stays until the Step's end is recorded.
#[tokio::test]
async fn a_retried_step_goes_on_in_the_session_it_already_prompted() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    agent(&sessions, &repo)
        .answer(task.clone())
        .await
        .expect("first attempt");

    let answer = agent(&sessions, &repo).answer(task).await.expect("retry");

    assert_eq!(answer.text, "Add a toggle to the settings page");
    assert_eq!(sessions.prepares(), 2);
    assert_eq!(sessions.landed(), 1);
    assert!(sessions.deleted.lock().expect("deleted").is_empty());
}

/// Once the Step's end is recorded, its session is deleted.
#[tokio::test]
async fn releasing_a_step_deletes_its_session() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let key = task.key;
    agent(&sessions, &repo).answer(task).await.expect("answer");

    agent(&sessions, &repo)
        .release(key, &julia())
        .await
        .expect("release");

    let claimed = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");
    let deleted = sessions.deleted.lock().expect("deleted");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].session_id.as_uuid(), claimed.session_id);
}

/// A session whose runtime disconnected before its prompt shows nothing of
/// the prompt either way: the Step asks, and the session takes it.
#[tokio::test(start_paused = true)]
async fn a_disconnected_session_that_never_took_its_prompt_is_prompted() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions {
        disconnected: true,
        ..FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
    };
    let task = plan_task(&repo).await;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(sessions.landed(), 1);
}

/// A prompt of unknown delivery may still land: one that never shows in
/// the session is never sent again, by this attempt or a retry.
#[tokio::test(start_paused = true)]
async fn prompts_whose_delivery_stays_unknown_fail_the_step_for_good() {
    let unknown = Reply {
        error: Some(RoutineSessionError::PromptDeliveryUnknown),
        lands: false,
    };
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .prompting(&[unknown]);

    let failed = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

    let Err(error) = failed else {
        panic!("no prompt showed");
    };
    assert!(matches!(error, SessionAgentError::PromptUnsettled));
    assert!(!error.retryable());
    assert_eq!(sessions.prompts(), 1);
}

/// Time spent opening and prompting the session counts too, so the Step
/// gives up on its session before Temporal times the attempt out.
#[tokio::test(start_paused = true)]
async fn the_session_deadline_counts_from_the_attempt_s_start() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(
        &repo,
        Some(the_plan()),
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    )
    .preparing(&[Reply {
        error: Some(RoutineSessionError::RuntimeUnavailable),
        lands: true,
    }]);
    *sessions.failing_reads.lock().expect("failing reads") = 12;
    let task = plan_task(&repo).await;
    let started = tokio::time::Instant::now();

    let failed = agent(&sessions, &repo).answer(task).await;

    assert!(matches!(failed, Err(SessionAgentError::TooSlow(_))));
    assert!(started.elapsed() <= super::SESSION_DEADLINE + super::LONGEST_WAIT);
}

/// A prepare that timed out at a gateway may still have opened the session:
/// its status settles that, and the Step goes on without preparing again.
#[tokio::test(start_paused = true)]
async fn a_prepare_that_failed_ambiguously_but_opened_the_session_goes_on() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[Reply {
            error: Some(RoutineSessionError::RuntimeUnavailable),
            lands: true,
        }]);
    let task = plan_task(&repo).await;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(sessions.prepares(), 1);
    assert_eq!(sessions.prompts(), 1);
}

/// A prompt whose delivery is unknown is not sent again when the session
/// shows it arrived.
#[tokio::test(start_paused = true)]
async fn a_prompt_of_unknown_delivery_that_arrived_is_not_sent_again() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .prompting(&[Reply {
            error: Some(RoutineSessionError::PromptDeliveryUnknown),
            lands: true,
        }]);
    let task = plan_task(&repo).await;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(sessions.prompts(), 1);
}

#[tokio::test]
async fn a_turn_that_ends_with_nothing_handed_back_fails_the_step_for_good() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, None, RoutineActionStatus::Succeeded);

    let failed = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

    let Err(error) = failed else {
        panic!("nothing was handed back");
    };
    assert!(matches!(error, SessionAgentError::NothingHandedBack));
    assert!(!error.retryable());
}

#[tokio::test]
async fn a_turn_that_fails_fails_the_step() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(
        &repo,
        Some(the_plan()),
        RoutineActionStatus::Failed(RoutineFailureReason::Refusal),
    );

    let failed = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

    assert!(matches!(
        failed,
        Err(SessionAgentError::TurnFailed(RoutineFailureReason::Refusal))
    ));
}

/// A prompt still on its way when the Step is stopped starts the turn
/// again: the stop is sent again until the session shows its turn ended.
#[tokio::test(start_paused = true)]
async fn stopping_a_step_stops_its_session_until_the_turn_ends() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions {
        ignored_stops: 1,
        ..FakeSessions::new(
            &repo,
            Some(the_plan()),
            RoutineActionStatus::Pending(RoutinePendingReason::Running),
        )
    };
    let task = plan_task(&repo).await;
    let key = task.key;
    // The Step's session is open and working on its prompt.
    repo.claim_session(
        key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
        },
        &task.outcomes,
    )
    .await
    .expect("claim");
    *sessions.open.lock().expect("open") = true;
    *sessions.prompted.lock().expect("prompted") = true;

    agent(&sessions, &repo)
        .stop(key, &julia())
        .await
        .expect("stop");

    let claimed = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");
    let cancelled = sessions.cancelled.lock().expect("cancelled").clone();
    assert_eq!(cancelled.len(), 2);
    assert_eq!(sessions.deleted.lock().expect("deleted").len(), 1);
    assert!(
        cancelled
            .iter()
            .all(|stop| stop.session_id.as_uuid() == claimed.session_id)
    );
}

/// A Step stopped before its session opened has nothing to stop: the stop
/// neither fails nor waits out the watch, and the delete is still tried.
#[tokio::test(start_paused = true)]
async fn stopping_a_step_whose_session_never_opened_succeeds() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
        },
        &task.outcomes,
    )
    .await
    .expect("claim");

    let started = tokio::time::Instant::now();

    agent(&sessions, &repo)
        .stop(task.key, &julia())
        .await
        .expect("stop");

    assert_eq!(sessions.deleted.lock().expect("deleted").len(), 1);
    assert!(
        started.elapsed() < super::STOP_WATCH,
        "{:?}",
        started.elapsed()
    );
}

/// An attempt Temporal gave up on may still run after the Step let go of
/// its session and deleted it: it must not prepare the session again, which
/// would open a fresh one under its id and start the work over.
#[tokio::test]
async fn a_step_that_let_go_of_its_session_never_opens_it_again() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    agent(&sessions, &repo)
        .answer(task.clone())
        .await
        .expect("answer");
    agent(&sessions, &repo)
        .release(task.key, &julia())
        .await
        .expect("release");

    let late = agent(&sessions, &repo).answer(task).await;

    assert!(matches!(late, Err(SessionAgentError::Released)), "{late:?}");
    assert_eq!(sessions.prepares(), 1);
    assert_eq!(sessions.landed(), 1);
}

/// The Step can let go of its session while an attempt opens it: the
/// attempt then sends no prompt.
#[tokio::test]
async fn a_session_let_go_while_it_opened_is_never_prompted() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions {
        releases_on_prepare: true,
        ..FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
    };

    let late = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

    assert!(matches!(late, Err(SessionAgentError::Released)), "{late:?}");
    assert_eq!(sessions.prompts(), 0);
}
