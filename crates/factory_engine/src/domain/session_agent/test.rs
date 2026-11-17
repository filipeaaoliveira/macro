use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agent_session::domain::error::AiAdmissionError;
use agent_session::domain::model::SessionVisibility;
use agent_session::domain::routines::{
    PrepareRoutineSession, PreparedRoutineSession, PromptRoutineSession, RoutineActionStatus,
    RoutineFailureReason, RoutinePendingReason, RoutinePromptAccepted, RoutineSessionAction,
    RoutineSessionError, RoutineSessions, ValidateRoutineSession, ValidatedRoutineSession,
};
use ai_billing::DenyReason;
use bot_id::BotId;
use factory_records::domain::models::{
    AgentName, EntryId, HandBack, NewRun, Outcome, PassedOver, Role, RunId, RunInsert, StepKey,
    StepSession,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{SessionAgent, SessionAgentError};
use crate::domain::agents::AgentDefinition;
use crate::domain::fakes::FakeRepo;
use crate::domain::flow::StepName;
use crate::domain::models::AgentTask;
use crate::domain::ports::{Retryable, StepAgent};

mod follow_up;

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
    /// How each later prompt is answered, in order; it works once these run
    /// out.
    follow_ups: Arc<Mutex<VecDeque<Reply>>>,
    open: Arc<Mutex<bool>>,
    prompted: Arc<Mutex<bool>>,
    prepared_calls: Arc<Mutex<Vec<PrepareRoutineSession>>>,
    prompt_calls: Arc<Mutex<Vec<PromptRoutineSession>>>,
    /// Prompts that landed in a session's history.
    landed: Arc<Mutex<usize>>,
    /// Reads of the status that fail before one works.
    failing_reads: Arc<Mutex<usize>>,
    /// Stops that fail before one works, as while the harness is down.
    failing_cancels: Arc<Mutex<usize>>,
    /// Whether a delete answers a session that is not there as deleted, as
    /// the routine sessions delete does.
    deletes_missing: bool,
    disconnected: bool,
    /// The Step lets go of its session while a prepare is in flight, as
    /// when Temporal gave the attempt up and another one ended the Step.
    releases_on_prepare: bool,
    /// The first prepare's Step is handed to a `backup` session while the
    /// prepare is in flight, as when an overlapping attempt passed it on.
    passes_on_prepare: Arc<Mutex<bool>>,
    cancelled: Arc<Mutex<Vec<RoutineSessionAction>>>,
    deleted: Arc<Mutex<Vec<RoutineSessionAction>>>,
    /// Stops the session ignores before its turn ends cancelled, as when a
    /// prompt on its way lands after the first stop.
    ignored_stops: usize,
    /// What the agent hands back for a follow-up, once it lands.
    follow_up_handed: Option<HandBack>,
    follow_up_calls: Arc<Mutex<Vec<PromptRoutineSession>>>,
}

impl FakeSessions {
    fn new(repo: &FakeRepo, handed: Option<HandBack>, ending: RoutineActionStatus) -> Self {
        Self {
            repo: repo.clone(),
            handed,
            ending,
            prepares: Arc::default(),
            prompts: Arc::default(),
            follow_ups: Arc::default(),
            open: Arc::default(),
            prompted: Arc::default(),
            prepared_calls: Arc::default(),
            prompt_calls: Arc::default(),
            landed: Arc::default(),
            failing_reads: Arc::default(),
            failing_cancels: Arc::default(),
            deletes_missing: false,
            disconnected: false,
            releases_on_prepare: false,
            passes_on_prepare: Arc::default(),
            cancelled: Arc::default(),
            deleted: Arc::default(),
            ignored_stops: 0,
            follow_up_handed: None,
            follow_up_calls: Arc::default(),
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

    fn following_up(self, replies: &[Reply]) -> Self {
        self.follow_ups.lock().expect("follow-ups").extend(replies);
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
        let passes = std::mem::take(&mut *self.passes_on_prepare.lock().expect("passes"));
        if passes {
            let key = self
                .repo
                .sessions
                .lock()
                .expect("sessions")
                .iter()
                .find(|claimed| claimed.session.session_id == session_id.as_uuid())
                .map(|claimed| claimed.key)
                .expect("a claimed session");
            let backup = StepSession {
                session_id: Uuid::now_v7(),
                action_id: Uuid::now_v7(),
                agent: Some(AgentName::try_from("backup".to_string()).expect("valid")),
                bot: Some(Uuid::from_u128(0xc0de)),
            };
            let passed = self
                .repo
                .pass_session(
                    key,
                    session_id.as_uuid(),
                    &Role::try_from("planner".to_string()).expect("valid"),
                    &PassedOver {
                        agent: AgentName::try_from("primary".to_string()).expect("valid"),
                        reason: "ai_allowance_exhausted".to_string(),
                    },
                    Some(&backup),
                )
                .await
                .expect("pass");
            assert!(passed);
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

    /// A follow-up lands once under its id; the same id again is refused.
    async fn follow_up(
        &self,
        command: PromptRoutineSession,
    ) -> Result<RoutinePromptAccepted, RoutineSessionError> {
        let action_id = command.action.action_id;
        let session_id = command.action.session_id.as_uuid();
        let again = {
            let mut calls = self.follow_up_calls.lock().expect("follow-ups");
            let again = calls.iter().any(|sent| sent.action.action_id == action_id);
            calls.push(command);
            again
        };
        if again {
            return Err(RoutineSessionError::Conflict);
        }
        let reply = self
            .follow_ups
            .lock()
            .expect("follow-ups")
            .pop_front()
            .unwrap_or(WORKS);
        if let Some(error) = reply.error {
            return Err(error);
        }
        if let Some(handed) = &self.follow_up_handed {
            self.repo
                .hand_back(session_id, handed)
                .await
                .expect("hand back");
        }
        Ok(RoutinePromptAccepted {
            action_id,
            queued: false,
        })
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
        {
            let mut failing = self.failing_cancels.lock().expect("failing cancels");
            if *failing > 0 {
                *failing -= 1;
                return Err(RoutineSessionError::RuntimeUnavailable);
            }
        }
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
            return if self.deletes_missing {
                Ok(())
            } else {
                Err(RoutineSessionError::OperationFailed)
            };
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
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Add a dark mode toggle",
            user_id: &julia(),
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
        })
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
        role: Role::try_from("planner".to_string()).expect("valid"),
        prompt: "Plan the change".to_string(),
        outcomes: vec![Outcome::done()],
        repo_url: None,
        answer_by: None,
    }
}

fn the_plan() -> HandBack {
    HandBack {
        outcome: Outcome::done(),
        output: "Add a toggle to the settings page".to_string(),
    }
}

fn planner() -> Role {
    Role::try_from("planner".to_string()).expect("valid")
}

/// The session the Step `key` works in.
async fn claimed_id(repo: &FakeRepo, key: StepKey) -> Uuid {
    repo.claimed_session(key)
        .await
        .expect("read")
        .expect("claimed")
        .session_id
}

fn name(agent: &str) -> AgentName {
    AgentName::try_from(agent.to_string()).expect("valid")
}

/// The bot whose sessions `primary` runs in.
const PRIMARY: BotId = BotId::new_from_uuid(Uuid::from_u128(0xc1a0));

/// The bot whose sessions `backup` runs in.
const BACKUP: BotId = BotId::new_from_uuid(Uuid::from_u128(0xc0de));

/// A planner is `primary`, on its own model and instructions, or `backup`
/// when `primary` cannot start.
fn agents() -> AgentDefinition {
    serde_json::from_value(serde_json::json!({
        "agents": {
            "primary": {
                "bot": Uuid::from_u128(0xc1a0),
                "model": "large-model",
                "instructions": "Keep each commit small."
            },
            "backup": { "bot": Uuid::from_u128(0xc0de) },
            "spare": { "bot": Uuid::from_u128(0x5a5e) }
        },
        "roles": { "planner": ["primary", "backup"] }
    }))
    .expect("a definition")
}

fn agent(sessions: &FakeSessions, repo: &FakeRepo) -> SessionAgent<FakeSessions, FakeRepo> {
    SessionAgent::new(sessions.clone(), repo.clone(), agents())
}

/// A refusal because the owner's quota for the agent is used up.
fn no_quota() -> Reply {
    Reply {
        error: Some(RoutineSessionError::Admission(AiAdmissionError::Denied(
            DenyReason::AllowanceExhausted,
        ))),
        lands: false,
    }
}

/// The Step's Role, Agent and the Agents passed over, as its record reads.
fn recorded(repo: &FakeRepo, key: StepKey) -> (Option<Role>, Option<AgentName>, Vec<PassedOver>) {
    let steps = repo.steps.lock().expect("steps");
    let (_, step) = steps
        .iter()
        .find(|(run, step)| *run == key.run_id && step.seq == key.seq)
        .expect("the step");
    (
        step.role.clone(),
        step.agent.clone(),
        step.passed_over.clone(),
    )
}

#[tokio::test]
async fn a_step_runs_in_a_session_of_its_owner_and_returns_what_the_agent_handed_back() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let key = task.key;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");
    let stopping = repo
        .stopping_session(key, std::time::Duration::from_secs(3600))
        .await
        .expect("read")
        .expect("claimed");

    assert_eq!(answer.outcome, Outcome::done());
    assert_eq!(answer.text, "Add a toggle to the settings page");
    // The prepare and the open are on record, for a stop to go by.
    assert!(stopping.opened && stopping.may_still_open);
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    assert_eq!(prepared.len(), 1);
    assert_eq!(prepared[0].selection.owner, julia());
    // The session is of the Role's first Agent, on its model and told its
    // instructions, and the Step keeps who took it.
    assert_eq!(prepared[0].selection.bot_id, PRIMARY);
    assert_eq!(prepared[0].selection.model.as_deref(), Some("large-model"));
    assert_eq!(
        prepared[0].instructions.as_deref(),
        Some("Keep each commit small.")
    );
    assert_eq!(
        recorded(&repo, key),
        (
            Some(Role::try_from("planner".to_string()).expect("valid")),
            Some(name("primary")),
            Vec::new()
        )
    );
    assert_eq!(prepared[0].visibility, SessionVisibility::Hidden);
    assert_eq!(prepared[0].repo_url, None);
    assert!(sessions.deleted.lock().expect("deleted").is_empty());
    let prompts = sessions.prompt_calls.lock().expect("prompts");
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].prompt.starts_with("Plan the change"));
    assert!(prompts[0].prompt.contains("macro.factory.handBack"));
    assert!(
        prompts[0]
            .prompt
            .contains("leave nothing running in the background")
    );
}

/// A Step of a Run that works on a repository opens its session in it, so
/// the session's runtime checks the repository out.
#[tokio::test]
async fn a_step_on_a_repository_opens_its_session_in_it() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = AgentTask {
        repo_url: Some("https://github.com/macro/app".to_string()),
        ..plan_task(&repo).await
    };

    agent(&sessions, &repo).answer(task).await.expect("answer");

    let prepared = sessions.prepared_calls.lock().expect("prepared");
    assert_eq!(
        prepared[0].repo_url.as_deref(),
        Some("https://github.com/macro/app")
    );
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
        .release(key, &planner(), &julia())
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
            agent: Some(name("primary")),
            bot: Some(PRIMARY.as_uuid()),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    // An attempt opened the session and saw it open.
    repo.record_open(key, claimed_id(&repo, key).await)
        .await
        .expect("open");
    *sessions.open.lock().expect("open") = true;
    *sessions.prompted.lock().expect("prompted") = true;

    agent(&sessions, &repo)
        .stop(key, &planner(), &julia())
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

/// A session an attempt saw open is never written off: a delete that fails,
/// however long ago its prepare was, fails the stop, which Temporal tries
/// again, for the catch-all error is as likely a timeout as a missing one.
#[tokio::test(start_paused = true)]
async fn stopping_a_step_whose_session_opened_is_tried_again_when_its_delete_fails() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let claimed_long_ago = uuid::Timestamp::from_unix(
        uuid::NoContext,
        u64::try_from((chrono::Utc::now() - chrono::TimeDelta::minutes(30)).timestamp())
            .expect("after the epoch"),
        0,
    );
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::new_v7(claimed_long_ago),
            action_id: Uuid::now_v7(),
            agent: Some(name("primary")),
            bot: Some(PRIMARY.as_uuid()),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    if let Some(claimed) = repo.sessions.lock().expect("sessions").first_mut() {
        claimed.prepared_at = Some(chrono::Utc::now() - chrono::TimeDelta::minutes(30));
        claimed.opened = true;
    }

    let stopped = agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await;

    let Err(error) = stopped else {
        panic!("a session that opened is never written off");
    };
    assert!(error.retryable());
    assert_eq!(sessions.deleted.lock().expect("deleted").len(), 1);
}

/// A session no attempt saw open, whose last prepare is older than any
/// takes to land, is not there: the stop neither waits out its watch nor
/// fails, so the Flow goes on.
#[tokio::test(start_paused = true)]
async fn stopping_a_step_whose_session_never_opened_ends_promptly() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: Some(name("primary")),
            bot: Some(PRIMARY.as_uuid()),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    if let Some(claimed) = repo.sessions.lock().expect("sessions").first_mut() {
        claimed.prepared_at = Some(chrono::Utc::now() - chrono::TimeDelta::minutes(30));
    }
    let started = tokio::time::Instant::now();

    agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await
        .expect("stop");

    assert!(
        started.elapsed() < super::STOP_WATCH,
        "{:?}",
        started.elapsed()
    );
    assert_eq!(sessions.deleted.lock().expect("deleted").len(), 1);
    assert!(
        !repo
            .holds_session(task.key, claimed_id(&repo, task.key).await)
            .await
            .expect("read"),
        "let go"
    );
}

/// A session whose prepare was sent lately may still open: a delete that
/// misses fails the stop, for Temporal to try again.
#[tokio::test(start_paused = true)]
async fn stopping_a_step_whose_prepare_was_sent_lately_is_tried_again() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: Some(name("primary")),
            bot: Some(PRIMARY.as_uuid()),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    repo.record_prepare(task.key, claimed_id(&repo, task.key).await)
        .await
        .expect("prepare");

    let stopped = agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await;

    let Err(error) = stopped else {
        panic!("a session that may still open is not written off");
    };
    assert!(error.retryable());
    assert_eq!(sessions.deleted.lock().expect("deleted").len(), 1);
}

/// A retry prepares again long after the Step claimed its session. Cancelled
/// while that prepare is in flight, the stop must not write the session off
/// by the claim's age: it is tried again, and deletes the session once the
/// prepare lands.
#[tokio::test(start_paused = true)]
async fn a_session_a_retry_prepares_as_the_run_is_cancelled_is_still_deleted() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let claimed_long_ago = uuid::Timestamp::from_unix(
        uuid::NoContext,
        u64::try_from((chrono::Utc::now() - chrono::TimeDelta::minutes(30)).timestamp())
            .expect("after the epoch"),
        0,
    );
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::new_v7(claimed_long_ago),
            action_id: Uuid::now_v7(),
            agent: Some(name("primary")),
            bot: Some(PRIMARY.as_uuid()),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    repo.record_prepare(task.key, claimed_id(&repo, task.key).await)
        .await
        .expect("the retry's prepare");

    let in_flight = agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await;
    *sessions.open.lock().expect("open") = true;
    let landed = agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await;

    assert!(in_flight.is_err_and(|error| error.retryable()));
    landed.expect("the session the prepare opened is deleted");
    assert!(!*sessions.open.lock().expect("open"));
}

/// A harness that fails a while answers neither the stop nor a read: the
/// stop keeps watching, stops the session once it answers, and deletes it.
#[tokio::test(start_paused = true)]
async fn stopping_a_step_rides_out_a_harness_that_fails_a_while() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(
        &repo,
        Some(the_plan()),
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    );
    let task = plan_task(&repo).await;
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: Some(name("primary")),
            bot: Some(PRIMARY.as_uuid()),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    // An attempt opened the session and saw it open.
    repo.record_open(task.key, claimed_id(&repo, task.key).await)
        .await
        .expect("open");
    *sessions.open.lock().expect("open") = true;
    *sessions.prompted.lock().expect("prompted") = true;
    *sessions.failing_cancels.lock().expect("failing cancels") = 2;
    *sessions.failing_reads.lock().expect("failing reads") = 2;

    agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await
        .expect("stop");

    assert!(!sessions.cancelled.lock().expect("cancelled").is_empty());
    assert_eq!(sessions.deleted.lock().expect("deleted").len(), 1);
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
        .release(task.key, &planner(), &julia())
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
    assert_eq!(
        sessions.deleted.lock().expect("deleted").len(),
        1,
        "the session it opened is deleted"
    );
}

/// A session that refuses or mismatches answers the same on every attempt:
/// the Step fails for good on the first, never preparing again.
#[tokio::test]
async fn a_refused_or_mismatched_session_fails_the_step_for_good() {
    for refusal in [
        RoutineSessionError::SessionMismatch,
        RoutineSessionError::ModelMismatch,
        RoutineSessionError::Forbidden,
    ] {
        let repo = FakeRepo::default();
        let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
            .preparing(&[Reply {
                error: Some(refusal),
                lands: false,
            }]);

        let failed = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

        let Err(error) = failed else {
            panic!("the session answered {refusal:?}");
        };
        assert!(!error.retryable(), "{error:?}");
        assert_eq!(sessions.prepares(), 1);
    }
}

/// A harness that is down, or failed, answers a prompt it never took: the
/// attempt fails in a way the next one may get past.
#[tokio::test]
async fn a_prompt_the_harness_was_down_for_is_tried_again() {
    for outage in [
        RoutineSessionError::RuntimeUnavailable,
        RoutineSessionError::OperationFailed,
    ] {
        let repo = FakeRepo::default();
        let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
            .prompting(&[Reply {
                error: Some(outage),
                lands: false,
            }]);

        let failed = agent(&sessions, &repo).answer(plan_task(&repo).await).await;

        let Err(error) = failed else {
            panic!("the harness answered {outage:?}");
        };
        assert!(error.retryable(), "{error:?}");
        assert_eq!(sessions.landed(), 0);
    }
}

/// A prepare refused outright opens nothing: a stop that follows need not
/// wait for it, and ends promptly.
#[tokio::test(start_paused = true)]
async fn a_step_whose_prepare_was_refused_stops_promptly() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[Reply {
            error: Some(RoutineSessionError::Forbidden),
            lands: false,
        }]);
    let task = plan_task(&repo).await;
    let key = task.key;
    let refused = agent(&sessions, &repo).answer(task).await;
    let started = tokio::time::Instant::now();

    let stopped = agent(&sessions, &repo)
        .stop(key, &planner(), &julia())
        .await;

    assert!(refused.is_err());
    stopped.expect("nothing to wait for");
    assert!(
        started.elapsed() < super::STOP_WATCH,
        "{:?}",
        started.elapsed()
    );
}

/// The routine sessions delete answers a session that is not there as
/// deleted. A stop while a prepare is in flight must not take that for done:
/// it is tried again, and deletes the session once the prepare opens it.
#[tokio::test(start_paused = true)]
async fn a_session_whose_prepare_is_in_flight_is_deleted_once_it_opens() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions {
        deletes_missing: true,
        ..FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
    };
    let task = plan_task(&repo).await;
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: Some(name("primary")),
            bot: Some(PRIMARY.as_uuid()),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    repo.record_prepare(task.key, claimed_id(&repo, task.key).await)
        .await
        .expect("prepare");

    let in_flight = agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await;
    *sessions.open.lock().expect("open") = true;
    let opened = agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await;

    assert!(in_flight.is_err_and(|error| error.retryable()));
    opened.expect("the session the prepare opened is deleted");
    assert!(!*sessions.open.lock().expect("open"));
}

/// An Agent whose owner's quota is used up is refused its session before
/// any work starts: the next Agent of the Role takes the Step in a session
/// of its own, and the Step keeps who was passed over and why.
#[tokio::test]
async fn an_agent_out_of_quota_passes_the_step_to_the_next_agent_of_its_role() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let task = plan_task(&repo).await;
    let key = task.key;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    let claimed = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed");
    assert_eq!(answer.text, "Add a toggle to the settings page");
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    let bots: Vec<BotId> = prepared.iter().map(|call| call.selection.bot_id).collect();
    assert_eq!(bots, [PRIMARY, BACKUP]);
    assert_ne!(prepared[0].session_id, prepared[1].session_id);
    assert_eq!(prepared[1].selection.model, None);
    assert_eq!(sessions.landed(), 1);
    assert_eq!(
        recorded(&repo, key).2,
        [PassedOver {
            agent: name("primary"),
            reason: "ai_allowance_exhausted".to_string(),
        }]
    );
    assert_eq!(recorded(&repo, key).1, Some(name("backup")));
    assert_eq!(claimed.session_id, prepared[1].session_id.as_uuid());
}

/// An Agent refused its prompt for lack of quota opened a session that did
/// no work: it is deleted, and the next Agent takes the Step.
#[tokio::test]
async fn an_agent_refused_its_prompt_for_lack_of_quota_leaves_no_session() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .prompting(&[no_quota()]);
    let task = plan_task(&repo).await;
    let key = task.key;

    agent(&sessions, &repo).answer(task).await.expect("answer");

    let deleted = sessions.deleted.lock().expect("deleted");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].bot_id, PRIMARY);
    let prompts = sessions.prompt_calls.lock().expect("prompts");
    let bots: Vec<BotId> = prompts.iter().map(|call| call.action.bot_id).collect();
    assert_eq!(bots, [PRIMARY, BACKUP]);
    assert_eq!(recorded(&repo, key).1, Some(name("backup")));
}

/// A retry goes on with the Agent that took the Step, and a stop stops that
/// Agent's session.
#[tokio::test(start_paused = true)]
async fn a_step_handed_on_is_retried_and_stopped_in_the_next_agent_s_session() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let task = plan_task(&repo).await;
    let key = task.key;
    agent(&sessions, &repo)
        .answer(task.clone())
        .await
        .expect("first attempt");

    agent(&sessions, &repo).answer(task).await.expect("retry");
    agent(&sessions, &repo)
        .stop(key, &planner(), &julia())
        .await
        .expect("stop");

    let prepared = sessions.prepared_calls.lock().expect("prepared");
    let bots: Vec<BotId> = prepared.iter().map(|call| call.selection.bot_id).collect();
    assert_eq!(bots, [PRIMARY, BACKUP, BACKUP]);
    assert_eq!(sessions.landed(), 1);
    assert_eq!(recorded(&repo, key).2.len(), 1);
    let deleted = sessions.deleted.lock().expect("deleted");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].bot_id, BACKUP);
}

/// When every Agent of the Role is out of quota, the Step fails for good,
/// having passed over each of them, the last one too.
#[tokio::test]
async fn a_step_whose_every_agent_is_out_of_quota_fails_for_good() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota(), no_quota()]);
    let task = plan_task(&repo).await;
    let key = task.key;

    let failed = agent(&sessions, &repo).answer(task).await;

    let Err(error) = failed else {
        panic!("no agent could start");
    };
    assert!(
        matches!(
            error,
            SessionAgentError::NoQuota(DenyReason::AllowanceExhausted)
        ),
        "{error:?}"
    );
    assert!(!error.retryable());
    assert_eq!(sessions.prepares(), 2);
    assert_eq!(recorded(&repo, key).1, Some(name("backup")));
    let passed: Vec<AgentName> = recorded(&repo, key)
        .2
        .into_iter()
        .map(|passed| passed.agent)
        .collect();
    assert_eq!(passed, [name("primary"), name("backup")]);
}

/// Only a lack of quota passes the Step on: any other refusal, and a quota
/// check that could not run, fail the Step's own Agent as before.
#[tokio::test(start_paused = true)]
async fn other_refusals_keep_the_step_with_its_agent() {
    for refusal in [
        RoutineSessionError::Forbidden,
        RoutineSessionError::PersonaUnavailable,
        RoutineSessionError::Admission(AiAdmissionError::Unavailable),
    ] {
        let repo = FakeRepo::default();
        let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
            .preparing(&[Reply {
                error: Some(refusal),
                lands: false,
            }]);
        let task = plan_task(&repo).await;
        let key = task.key;

        let failed = agent(&sessions, &repo).answer(task).await;

        assert!(
            matches!(failed, Err(SessionAgentError::Session(error)) if error == refusal),
            "{refusal:?}"
        );
        assert_eq!(sessions.prepares(), 1, "{refusal:?}");
        assert_eq!(recorded(&repo, key).1, Some(name("primary")), "{refusal:?}");
        assert!(recorded(&repo, key).2.is_empty(), "{refusal:?}");
    }
}

/// A Role the agent definition does not fill fails the Step for good.
#[tokio::test]
async fn a_role_no_agent_fills_fails_the_step_for_good() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = AgentTask {
        role: Role::try_from("reviewer".to_string()).expect("valid"),
        ..plan_task(&repo).await
    };

    let failed = agent(&sessions, &repo).answer(task).await;

    let Err(error) = failed else {
        panic!("no agent reviews");
    };
    assert!(matches!(error, SessionAgentError::UnfilledRole(_)));
    assert!(!error.retryable());
    assert_eq!(sessions.prepares(), 0);
}

/// An attempt whose session the Step handed to another Agent while it
/// opened never prompts it: it deletes the session it opened and goes on in
/// the session the Step works in now.
#[tokio::test]
async fn a_session_handed_on_while_it_opened_is_never_prompted() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    *sessions.passes_on_prepare.lock().expect("passes") = true;
    let task = plan_task(&repo).await;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.text, "Add a toggle to the settings page");
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    let deleted = sessions.deleted.lock().expect("deleted");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].bot_id, PRIMARY);
    assert_eq!(deleted[0].session_id, prepared[0].session_id);
    let prompts = sessions.prompt_calls.lock().expect("prompts");
    let prompted: Vec<BotId> = prompts.iter().map(|call| call.action.bot_id).collect();
    assert_eq!(
        prompted,
        [BACKUP],
        "the session handed on is never prompted"
    );
}

/// An attempt out of quota whose Step another attempt already handed on goes
/// on in the next Agent's session, and records no second pass.
#[tokio::test]
async fn an_attempt_finds_its_step_handed_on_by_another_and_goes_on_there() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    *sessions.passes_on_prepare.lock().expect("passes") = true;
    let task = plan_task(&repo).await;
    let key = task.key;

    let answer = agent(&sessions, &repo).answer(task).await.expect("answer");

    assert_eq!(answer.text, "Add a toggle to the settings page");
    let prepared = sessions.prepared_calls.lock().expect("prepared");
    let bots: Vec<BotId> = prepared.iter().map(|call| call.selection.bot_id).collect();
    assert_eq!(bots, [PRIMARY, BACKUP]);
    assert_eq!(recorded(&repo, key).1, Some(name("backup")));
    assert_eq!(recorded(&repo, key).2.len(), 1);
}

/// A session claimed before Steps recorded their Agent is of the first
/// Agent of the Step's Role: a stop stops that Agent's session.
#[tokio::test(start_paused = true)]
async fn a_session_claimed_with_no_agent_is_stopped_as_the_role_s_first_agent() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: None,
            bot: None,
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    repo.record_prepare(task.key, claimed_id(&repo, task.key).await)
        .await
        .expect("prepare");
    repo.record_open(task.key, claimed_id(&repo, task.key).await)
        .await
        .expect("open");
    *sessions.open.lock().expect("open") = true;

    agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await
        .expect("stop");

    let deleted = sessions.deleted.lock().expect("deleted");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].bot_id, PRIMARY);
}

/// A session's bot is kept with it: a stop deletes it as its bot even once
/// the agent definition no longer names its Agent.
#[tokio::test(start_paused = true)]
async fn a_session_whose_agent_was_removed_is_still_stopped() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded);
    let task = plan_task(&repo).await;
    let gone = Uuid::from_u128(0x6033);
    repo.claim_session(
        task.key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: Some(name("retired")),
            bot: Some(gone),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");
    repo.record_open(task.key, claimed_id(&repo, task.key).await)
        .await
        .expect("open");
    *sessions.open.lock().expect("open") = true;

    agent(&sessions, &repo)
        .stop(task.key, &planner(), &julia())
        .await
        .expect("stop");

    let deleted = sessions.deleted.lock().expect("deleted");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].bot_id.as_uuid(), gone);
}

/// A Step whose Agent the Role no longer lists, out of quota, goes to the
/// Role's first Agent now.
#[tokio::test]
async fn an_agent_the_role_no_longer_lists_passes_the_step_to_its_first() {
    let repo = FakeRepo::default();
    let sessions = FakeSessions::new(&repo, Some(the_plan()), RoutineActionStatus::Succeeded)
        .preparing(&[no_quota()]);
    let task = plan_task(&repo).await;
    let key = task.key;
    repo.claim_session(
        key,
        StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: Some(name("spare")),
            bot: Some(Uuid::from_u128(0x5a5e)),
        },
        &task.role,
        &task.outcomes,
    )
    .await
    .expect("claim");

    agent(&sessions, &repo).answer(task).await.expect("answer");

    let prepared = sessions.prepared_calls.lock().expect("prepared");
    let bots: Vec<BotId> = prepared.iter().map(|call| call.selection.bot_id).collect();
    assert_eq!(
        bots,
        [BotId::new_from_uuid(Uuid::from_u128(0x5a5e)), PRIMARY]
    );
    assert_eq!(recorded(&repo, key).1, Some(name("primary")));
}
