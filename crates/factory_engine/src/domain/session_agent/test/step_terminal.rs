//! A Step's work and its owner's Step Terminal: the wait on the agent's
//! turn follows a note and a resume the owner sent, holds while the Step is
//! paused (its time not counted), stops the turn again while paused, and
//! ends at once on a stop; and the Terminal reaches the Step's session.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_session::domain::routines::{
    PrepareRoutineSession, PreparedRoutineSession, PromptRoutineSession, RoutineActionStatus,
    RoutineFailureReason, RoutinePendingReason, RoutinePromptAccepted, RoutineSessionAction,
    RoutineSessionError, RoutineSessions, ValidateRoutineSession, ValidatedRoutineSession,
};
use factory_records::domain::models::{HandBack, NewSteer, Outcome, StepKey, StepSession};
use factory_records::domain::ports::RunRepository;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::{agents, julia, plan_task, planner, the_plan};
use crate::domain::fakes::FakeRepo;
use crate::domain::models::{AgentAnswer, AgentTask, AgentTurn, SessionSteer};
use crate::domain::ports::{PauseNotices, Retryable, StepAgent};
use crate::domain::session_agent::{SESSION_DEADLINE, SessionAgent, SessionAgentError};
use crate::domain::step_terminal::PAUSED_STEP_ENDS_AFTER;

/// A session whose turns end only when the test says: each prompt's
/// status is what the test set for its action, `Running` once the first
/// prompt lands and `AwaitingPrompt` for a later prompt until then. Keeps
/// the later prompts and the stops it was sent; refuses later prompts when
/// `refuse` is set.
#[derive(Clone, Default)]
struct ScriptedSessions {
    statuses: Arc<Mutex<HashMap<Uuid, RoutineActionStatus>>>,
    opened: Arc<Mutex<bool>>,
    follow_ups: Arc<Mutex<Vec<PromptRoutineSession>>>,
    cancels: Arc<Mutex<Vec<RoutineSessionAction>>>,
    refuse: Option<RoutineSessionError>,
    /// What the agent said in each prompt's turn.
    replies: Arc<Mutex<HashMap<Uuid, String>>>,
}

impl ScriptedSessions {
    fn set(&self, action: Uuid, status: RoutineActionStatus) {
        self.statuses
            .lock()
            .expect("statuses")
            .insert(action, status);
    }

    fn said(&self, action: Uuid, words: &str) {
        self.replies
            .lock()
            .expect("replies")
            .insert(action, words.to_string());
    }

    fn cancels(&self) -> usize {
        self.cancels.lock().expect("cancels").len()
    }
}

impl RoutineSessions for ScriptedSessions {
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
        let mut opened = self.opened.lock().expect("opened");
        if *opened {
            return Err(RoutineSessionError::Conflict);
        }
        *opened = true;
        Ok(PreparedRoutineSession {
            session_id: command.session_id,
        })
    }

    async fn prompt(
        &self,
        command: PromptRoutineSession,
    ) -> Result<RoutinePromptAccepted, RoutineSessionError> {
        let action_id = command.action.action_id;
        self.statuses
            .lock()
            .expect("statuses")
            .entry(action_id.as_uuid())
            .or_insert(RoutineActionStatus::Pending(RoutinePendingReason::Running));
        Ok(RoutinePromptAccepted {
            action_id,
            queued: false,
        })
    }

    async fn follow_up(
        &self,
        command: PromptRoutineSession,
    ) -> Result<RoutinePromptAccepted, RoutineSessionError> {
        if let Some(error) = self.refuse {
            return Err(error);
        }
        let action_id = command.action.action_id;
        self.follow_ups.lock().expect("follow-ups").push(command);
        Ok(RoutinePromptAccepted {
            action_id,
            queued: true,
        })
    }

    async fn status(
        &self,
        command: RoutineSessionAction,
    ) -> Result<RoutineActionStatus, RoutineSessionError> {
        Ok(self
            .statuses
            .lock()
            .expect("statuses")
            .get(&command.action_id.as_uuid())
            .copied()
            .unwrap_or(RoutineActionStatus::Pending(
                RoutinePendingReason::AwaitingPrompt,
            )))
    }

    async fn cancel(&self, command: RoutineSessionAction) -> Result<(), RoutineSessionError> {
        self.cancels.lock().expect("cancels").push(command);
        Ok(())
    }

    async fn reply(
        &self,
        command: RoutineSessionAction,
    ) -> Result<Option<String>, RoutineSessionError> {
        Ok(self
            .replies
            .lock()
            .expect("replies")
            .get(&command.action_id.as_uuid())
            .cloned())
    }

    async fn delete(&self, _command: RoutineSessionAction) -> Result<(), RoutineSessionError> {
        Ok(())
    }
}

/// The Step's work on its own task, and once its first prompt landed, the
/// Step, its session, and the agent to steer it with.
struct Working {
    work: JoinHandle<Result<AgentAnswer, SessionAgentError>>,
    task: AgentTask,
    key: StepKey,
    session: StepSession,
    agent: SessionAgent<ScriptedSessions, FakeRepo>,
}

async fn working(repo: &FakeRepo, sessions: &ScriptedSessions) -> Working {
    let agent = SessionAgent::new(sessions.clone(), repo.clone(), agents());
    let task = plan_task(repo).await;
    let key = task.key;
    let work = tokio::spawn({
        let agent = agent.clone();
        let task = task.clone();
        async move { agent.answer(task).await }
    });
    // Let the work open the session and send its first prompt.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let session = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("the step claimed its session");
    Working {
        work,
        task,
        key,
        session,
        agent,
    }
}

impl Working {
    /// The owner's `steer`, kept and sent as the Run API does.
    async fn steer(&self, repo: &FakeRepo, steer: NewSteer, act: SessionSteer) {
        repo.steer(self.key, &steer)
            .await
            .expect("records")
            .expect("the step takes it");
        self.agent
            .steer(self.key, &planner(), &julia(), &self.session, act)
            .await
            .expect("the session takes it");
    }

    async fn note(&self, repo: &FakeRepo, note: &str) -> Uuid {
        let prompt_id = Uuid::now_v7();
        self.steer(
            repo,
            NewSteer::Note {
                note: note.to_string(),
                prompt_id,
            },
            SessionSteer::Prompt {
                prompt_id,
                prompt: note.to_string(),
            },
        )
        .await;
        prompt_id
    }

    async fn hand_back(&self, repo: &FakeRepo, handed: &HandBack) {
        repo.hand_back(self.session.session_id, handed)
            .await
            .expect("hand back");
    }
}

fn revised() -> HandBack {
    HandBack {
        outcome: Outcome::done(),
        output: "Add the toggle, with the theme tokens".to_string(),
    }
}

/// A note sent while the agent works is a later prompt of its session; the
/// Step does not end with the turn before it, but with the note's turn,
/// and the agent's latest hand back.
#[tokio::test(start_paused = true)]
async fn the_step_waits_for_the_agent_to_answer_a_note() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;

    let note = working.note(&repo, "Use the theme tokens").await;
    let sent = sessions.follow_ups.lock().expect("follow-ups").clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].action.action_id.as_uuid(), note);
    assert_eq!(
        sent[0].action.session_id.as_uuid(),
        working.session.session_id
    );
    assert_eq!(sent[0].prompt, "Use the theme tokens");

    working.hand_back(&repo, &the_plan()).await;
    sessions.set(working.session.action_id, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(
        !working.work.is_finished(),
        "the turn before the note is not the step's end"
    );

    sessions.set(
        note,
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    );
    tokio::time::sleep(Duration::from_secs(60)).await;
    working.hand_back(&repo, &revised()).await;
    sessions.set(note, RoutineActionStatus::Succeeded);
    let answer = working.work.await.expect("joined").expect("answer");
    assert_eq!(answer.text, revised().output);
    assert!(
        repo.steer(
            working.key,
            &NewSteer::Note {
                note: "Too late".to_string(),
                prompt_id: Uuid::now_v7(),
            },
        )
        .await
        .expect("records")
        .is_err(),
        "the step takes no note once its work ended its wait"
    );
}

/// A pause stops the agent's turn and holds the Step past the time its
/// wait would have taken; a turn that starts again while it is paused is
/// stopped; the resume's turn is the Step's.
#[tokio::test(start_paused = true)]
async fn a_paused_step_holds_until_it_is_resumed() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;

    working
        .steer(&repo, NewSteer::Pause, SessionSteer::Interrupt)
        .await;
    assert_eq!(sessions.cancels(), 1);
    // The stop was lost: the work stops the turn again while paused.
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(sessions.cancels() > 1, "a running turn is stopped again");
    sessions.set(
        working.session.action_id,
        RoutineActionStatus::Failed(RoutineFailureReason::Cancelled),
    );
    let stops = sessions.cancels();

    tokio::time::sleep(SESSION_DEADLINE + Duration::from_secs(10 * 60)).await;
    assert!(
        !working.work.is_finished(),
        "the time paused does not count against the wait"
    );
    assert_eq!(sessions.cancels(), stops, "a stopped turn is left alone");

    let resume = Uuid::now_v7();
    working
        .steer(
            &repo,
            NewSteer::Resume {
                note: None,
                prompt_id: resume,
            },
            SessionSteer::Prompt {
                prompt_id: resume,
                prompt: "Go on".to_string(),
            },
        )
        .await;
    sessions.set(
        resume,
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    );
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(!working.work.is_finished());
    working.hand_back(&repo, &the_plan()).await;
    sessions.set(resume, RoutineActionStatus::Succeeded);
    let answer = working.work.await.expect("joined").expect("answer");
    assert_eq!(answer.text, the_plan().output);
}

/// A stop ends the Step's wait at once, failing it for good, and stops the
/// turn, even a turn that would have ended well.
#[tokio::test(start_paused = true)]
async fn a_stop_ends_the_step_at_once() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;

    working
        .steer(&repo, NewSteer::Stop, SessionSteer::Interrupt)
        .await;
    let stopped = working.work.await.expect("joined");

    let Err(error) = stopped else {
        panic!("a stopped step fails");
    };
    assert!(
        matches!(error, SessionAgentError::StoppedByOwner),
        "{error:?}"
    );
    assert!(!error.retryable());
    assert!(
        sessions.cancels() >= 2,
        "the terminal's stop and the work's"
    );
}

/// The Terminal's prompt goes to the Step's session as a later prompt, run
/// by the bot recorded with it; a prompt whose delivery is unknown may
/// land, so it is taken, while a refusal is not.
#[tokio::test]
async fn the_terminal_reaches_the_step_s_session() {
    let repo = FakeRepo::default();
    let session = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: None,
        bot: Some(Uuid::from_u128(0xc0d1)),
    };
    let key = StepKey {
        run_id: factory_records::domain::models::RunId::generate(),
        seq: 1,
    };
    let prompt = |prompt_id| SessionSteer::Prompt {
        prompt_id,
        prompt: "Use tokens".to_string(),
    };

    let sessions = ScriptedSessions::default();
    let agent = SessionAgent::new(sessions.clone(), repo.clone(), agents());
    agent
        .steer(key, &planner(), &julia(), &session, prompt(Uuid::now_v7()))
        .await
        .expect("taken");
    agent
        .steer(key, &planner(), &julia(), &session, SessionSteer::Interrupt)
        .await
        .expect("stopped");
    let sent = sessions.follow_ups.lock().expect("follow-ups")[0].clone();
    assert_eq!(sent.action.bot_id.as_uuid(), Uuid::from_u128(0xc0d1));
    assert_eq!(sent.action.owner, julia());
    assert_eq!(sessions.cancels(), 1);

    for (refusal, taken) in [
        (RoutineSessionError::PromptDeliveryUnknown, true),
        (RoutineSessionError::Conflict, true),
        (RoutineSessionError::Forbidden, false),
        (RoutineSessionError::RuntimeUnavailable, false),
    ] {
        let refusing = ScriptedSessions {
            refuse: Some(refusal),
            ..ScriptedSessions::default()
        };
        let agent = SessionAgent::new(refusing, repo.clone(), agents());
        let steered = agent
            .steer(key, &planner(), &julia(), &session, prompt(Uuid::now_v7()))
            .await;
        assert_eq!(steered.is_ok(), taken, "{refusal:?}");
    }
}

/// A note the session refused never arrives: once the Step no longer
/// follows it, the wait goes back to the turn it followed before, and the
/// Step ends with it rather than wait for the note.
#[tokio::test(start_paused = true)]
async fn the_step_stops_waiting_for_a_note_the_session_refused() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;

    let note = working.note(&repo, "Use the theme tokens").await;
    working.hand_back(&repo, &the_plan()).await;
    sessions.set(working.session.action_id, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(!working.work.is_finished(), "the wait follows the note");

    // The Run API found the session refused it, after the wait took it on.
    repo.unsteer(working.key, note, None)
        .await
        .expect("unsteer");
    tokio::time::sleep(Duration::from_secs(60)).await;
    let answer = working.work.await.expect("joined").expect("answer");
    assert_eq!(answer.text, the_plan().output);
}

/// A note the owner sent during the Step's work is not followed once the
/// factory sent the session a prompt of its own: that prompt's failed turn
/// fails it, whatever the note's turn did.
#[tokio::test(start_paused = true)]
async fn a_leftover_note_does_not_hide_a_failed_factory_turn() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;
    let note = working.note(&repo, "Use the theme tokens").await;
    working.hand_back(&repo, &the_plan()).await;
    sessions.set(working.session.action_id, RoutineActionStatus::Succeeded);
    sessions.set(note, RoutineActionStatus::Succeeded);
    working.work.await.expect("joined").expect("answer");

    let fix = Uuid::now_v7();
    sessions.set(
        fix,
        RoutineActionStatus::Failed(RoutineFailureReason::RuntimeError),
    );
    let fixed = working
        .agent
        .follow_up(AgentTurn {
            key: working.key,
            owner: julia(),
            role: planner(),
            action_id: fix,
            place: 3,
            prompt: "Fix the checks".to_string(),
            outcomes: vec![Outcome::done()],
            answer_by: chrono::Utc::now() + chrono::TimeDelta::minutes(30),
        })
        .await;

    assert!(
        matches!(fixed, Err(SessionAgentError::TurnFailed(_))),
        "{fixed:?}"
    );
    let sent = sessions.follow_ups.lock().expect("follow-ups").clone();
    assert!(
        sent.iter().all(|prompt| prompt.out_of_sight),
        "factory prompts are out of sight"
    );
}

/// A Step paused near the end of its work's time tells its owner once, and
/// ends failed when the time runs out, recording why.
#[tokio::test(start_paused = true)]
async fn a_step_paused_past_its_time_ends_saying_why() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let told = Arc::new(Mutex::new(Vec::new()));
    let agent = SessionAgent::new(sessions.clone(), repo.clone(), agents())
        .with_pause_notices(Told(told.clone()));
    let task = plan_task(&repo).await;
    let key = task.key;
    // The Step started long ago: its time is nearly out.
    let started = chrono::Utc::now() - PAUSED_STEP_ENDS_AFTER + chrono::TimeDelta::minutes(5);
    repo.steps
        .lock()
        .expect("steps")
        .iter_mut()
        .filter(|(run, step)| *run == key.run_id && step.seq == key.seq)
        .for_each(|(_, step)| step.started_at = started);
    let work = tokio::spawn({
        let agent = agent.clone();
        async move { agent.answer(task).await }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    repo.steer(key, &NewSteer::Pause)
        .await
        .expect("records")
        .expect("pause");
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(
        *told.lock().expect("told"),
        [(key, started + PAUSED_STEP_ENDS_AFTER)],
        "the owner is told once"
    );
    assert!(!work.is_finished());

    // The wall clock is the work's: move the Step's start back past it.
    repo.steps
        .lock()
        .expect("steps")
        .iter_mut()
        .filter(|(run, step)| *run == key.run_id && step.seq == key.seq)
        .for_each(|(_, step)| step.started_at = started - chrono::TimeDelta::minutes(10));
    let ended = work.await.expect("joined");
    assert!(
        matches!(ended, Err(SessionAgentError::PausedTooLong)),
        "{ended:?}"
    );
    assert!(repo.steering(key).await.expect("read").pause_ran_out);
    assert_eq!(told.lock().expect("told").len(), 1);
}

/// A paused Step and when its time runs out, as its owner was told.
type Notice = (StepKey, chrono::DateTime<chrono::Utc>);

/// Keeps who was told a paused Step's time runs out.
struct Told(Arc<Mutex<Vec<Notice>>>);

impl PauseNotices for Told {
    fn running_out(
        &self,
        key: StepKey,
        ends_by: chrono::DateTime<chrono::Utc>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.0.lock().expect("told").push((key, ends_by));
        Box::pin(async {})
    }
}

/// The reminder of an agent whose turn ended without a result is a factory
/// prompt, sent out of sight, after which the owner steers the Step again:
/// the wait follows their note sent during the reminder's turn, and an
/// agent that hands nothing back in it either stops with what it said in
/// the note's turn, its last.
#[tokio::test(start_paused = true)]
async fn the_owner_steers_the_reminder_and_the_note_has_the_last_words() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;

    sessions.set(working.session.action_id, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(60)).await;
    let sent = sessions.follow_ups.lock().expect("follow-ups").clone();
    assert_eq!(sent.len(), 1, "the agent is reminded once");
    let reminder = sent[0].action.action_id.as_uuid();
    assert!(
        sent[0]
            .prompt
            .starts_with("## Your turn ended without a result")
    );
    assert!(sent[0].out_of_sight, "a factory prompt follows no one");
    sessions.set(
        reminder,
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    );

    let note = working.note(&repo, "Say what blocks you").await;
    sessions.said(reminder, "Still looking.");
    sessions.set(reminder, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(
        !working.work.is_finished(),
        "the reminder's turn is not the end while a note follows it"
    );

    sessions.said(note, "I need the API key.");
    sessions.set(note, RoutineActionStatus::Succeeded);
    let ended = working.work.await.expect("joined");

    match ended {
        Err(SessionAgentError::StoppedWithoutResult(said)) => {
            assert_eq!(said, "I need the API key.");
        }
        other => panic!("the agent stopped without a result: {other:?}"),
    }
}

/// A pause during the reminder's turn holds the Step, which no ending turn
/// ends meanwhile; the owner's stop then ends it as stopped by them, not as
/// an agent that stopped without a result, so no Gate asks them about it.
#[tokio::test(start_paused = true)]
async fn a_reminded_step_pauses_and_stops_as_its_owner_says() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;

    sessions.set(working.session.action_id, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(60)).await;
    let reminder = sessions.follow_ups.lock().expect("follow-ups")[0]
        .action
        .action_id
        .as_uuid();
    working
        .steer(&repo, NewSteer::Pause, SessionSteer::Interrupt)
        .await;
    sessions.set(reminder, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert!(
        !working.work.is_finished(),
        "the turn the pause stopped is not the step's end"
    );

    working
        .steer(&repo, NewSteer::Stop, SessionSteer::Interrupt)
        .await;
    let stopped = working.work.await.expect("joined");

    assert!(
        matches!(stopped, Err(SessionAgentError::StoppedByOwner)),
        "{stopped:?}"
    );
}

/// The reminder sent once the first turn ended with nothing handed back,
/// the turn of the Step's first prompt having ended.
async fn reminded(sessions: &ScriptedSessions, working: &Working) -> Uuid {
    sessions.set(working.session.action_id, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(60)).await;
    let reminder = sessions.follow_ups.lock().expect("follow-ups")[0]
        .action
        .action_id
        .as_uuid();
    sessions.set(
        reminder,
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    );
    reminder
}

/// An attempt retried while the reminder's turn runs (Temporal lost the
/// first attempt) does not wait on the first prompt, whose turn ended, nor
/// close the steering the reminder's turn takes: the owner still steers
/// it, and both attempts end with what the agent hands back to it.
#[tokio::test(start_paused = true)]
async fn a_retried_attempt_waits_on_the_reminder_and_leaves_it_steerable() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;
    let reminder = reminded(&sessions, &working).await;

    let retried = tokio::spawn({
        let agent = working.agent.clone();
        let task = working.task.clone();
        async move { agent.answer(task).await }
    });
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(!retried.is_finished(), "the retry waits on the reminder");
    let note = working.note(&repo, "Hand back what you have").await;

    sessions.set(reminder, RoutineActionStatus::Succeeded);
    sessions.set(
        note,
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    );
    tokio::time::sleep(Duration::from_secs(60)).await;
    working.hand_back(&repo, &the_plan()).await;
    sessions.set(note, RoutineActionStatus::Succeeded);

    let first = working.work.await.expect("joined").expect("answer");
    let again = retried.await.expect("joined").expect("answer");
    assert_eq!(first.text, the_plan().output);
    assert_eq!(again.text, the_plan().output);
}

/// The reminder's wait keeps the time the Step was paused before it: a
/// Step paused past its attempt's time, then resumed, is still reminded
/// and waited on, not failed at once for its time.
#[tokio::test(start_paused = true)]
async fn a_reminder_keeps_the_time_its_step_was_paused() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;
    working
        .steer(&repo, NewSteer::Pause, SessionSteer::Interrupt)
        .await;
    sessions.set(
        working.session.action_id,
        RoutineActionStatus::Failed(RoutineFailureReason::Cancelled),
    );
    tokio::time::sleep(SESSION_DEADLINE + Duration::from_secs(10 * 60)).await;
    let resume = Uuid::now_v7();
    working
        .steer(
            &repo,
            NewSteer::Resume {
                note: None,
                prompt_id: resume,
            },
            SessionSteer::Prompt {
                prompt_id: resume,
                prompt: "Go on".to_string(),
            },
        )
        .await;
    sessions.set(resume, RoutineActionStatus::Succeeded);
    tokio::time::sleep(Duration::from_secs(60)).await;

    let sent = sessions.follow_ups.lock().expect("follow-ups").clone();
    let reminder = sent
        .iter()
        .map(|prompt| prompt.action.action_id.as_uuid())
        .find(|sent| *sent != resume)
        .expect("the agent is reminded");
    sessions.set(
        reminder,
        RoutineActionStatus::Pending(RoutinePendingReason::Running),
    );
    tokio::time::sleep(Duration::from_secs(5 * 60)).await;
    assert!(
        !working.work.is_finished(),
        "the reminder is waited on within the time paused"
    );
    working.hand_back(&repo, &the_plan()).await;
    sessions.set(reminder, RoutineActionStatus::Succeeded);
    let answer = working.work.await.expect("joined").expect("answer");
    assert_eq!(answer.text, the_plan().output);
}

/// A reminder whose turn still runs when the attempt's time is up is a
/// later prompt still going: the attempt fails to be retried, which waits
/// on the same turn, rather than failing the Step for good.
#[tokio::test(start_paused = true)]
async fn a_reminder_still_going_at_the_attempt_s_end_is_retried() {
    let repo = FakeRepo::default();
    let sessions = ScriptedSessions::default();
    let working = working(&repo, &sessions).await;
    reminded(&sessions, &working).await;

    tokio::time::sleep(SESSION_DEADLINE + Duration::from_secs(60)).await;
    let ended = working.work.await.expect("joined");

    let Err(error) = ended else {
        panic!("the attempt's time ran out");
    };
    assert!(matches!(error, SessionAgentError::Overdue), "{error:?}");
    assert!(error.retryable());
}
