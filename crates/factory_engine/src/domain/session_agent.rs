//! A [`StepAgent`] that does a Step's work in a Macro agent session, run as
//! the Run's owner through the routine sessions capability, and reads the
//! result the agent hands back from it. The session is of the first Agent
//! the agent definition lists for the Step's Role; when an Agent cannot start
//! for lack of quota, the next one takes the Step.

#[cfg(test)]
mod test;

use std::time::Duration;

use agent_runtime_protocol::domain::action::AgentActionId;
use agent_session::domain::error::AiAdmissionError;
use agent_session::domain::model::{AgentSessionId, SessionSource, SessionVisibility};
use agent_session::domain::routines::{
    ExecRoutineSession, PrepareRoutineSession, PromptRoutineSession, RoutineActionStatus,
    RoutineFailureReason, RoutinePendingReason, RoutinePromptAccepted, RoutineSessionAction,
    RoutineSessionError, RoutineSessions, ValidateRoutineSession,
};
use ai_billing::DenyReason;
use bot_id::BotId;
use factory_records::domain::models::{
    AgentName, HandedBack, Outcome, PassedOver, Role, StepKey, StepSession,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use trigger_context::TriggerOrigin;
use uuid::Uuid;

use crate::domain::agents::{Agent, AgentDefinition};
use agent_session::domain::session_command::SessionCommandOutput;

use crate::domain::models::{AgentAnswer, AgentCommand, AgentTask, AgentTurn, SessionSteer};
use crate::domain::ports::PauseNotices;
use crate::domain::ports::{AgentFailure, Retryable, StepAgent};
use crate::domain::step_terminal::{PAUSE_WARNING, paused_step_ends_by};

/// What the agent is told about handing its result back.
const HAND_BACK: &str = include_str!("session_agent/hand_back.md");

/// How long to wait before the first look at the session, and the longest
/// wait between two looks.
const FIRST_LOOK: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(10);

/// How long an ambiguous prepare is given to show its session open: past the
/// longest a machine's agent takes to open one.
const OPEN_SETTLE: Duration = Duration::from_secs(200);

/// How long after a prepare was sent its session may still open: past the
/// routine sessions route's 30 s and [`OPEN_SETTLE`], with a margin. A stop
/// writes off a session it can't delete only when none opened and the last
/// prepare is older than this. A runtime that opens a requested session
/// later still, as macrod may when its event stream lags or replays, leaves
/// a session no stop deletes: the record narrows that, no bound closes it.
const PREPARE_LANDS_WITHIN: Duration = Duration::from_secs(5 * 60);

/// How long a prompt whose delivery is unknown is given to show in the
/// session.
const PROMPT_SETTLE: Duration = Duration::from_secs(60);

/// How long a session resumed after its Step's question may show its
/// runtime disconnected before it counts as lost: past the harness's start
/// of a stopped sandbox and its restore of the session.
const RESUME_SETTLE: Duration = Duration::from_secs(5 * 60);

/// Why a Step that went on from its question left the asking Step's
/// session, as its passed-over Agent records it.
pub const SESSION_LOST: &str = "session_lost";

/// How long a stopped Step's session is watched, to stop it again if a
/// prompt still on its way started it.
const STOP_WATCH: Duration = Duration::from_secs(60);

/// How many reads of the session's status in a row may fail before the
/// Step gives up on it.
const MAX_FAILED_READS: u8 = 5;

/// How long an attempt of a Step may take before it gives up on its session,
/// counted from the attempt's start and short of the Step's own timeout, so
/// the failure says why.
const SESSION_DEADLINE: Duration = Duration::from_secs(29 * 60);

/// Why a Step's session did not hand back a result.
#[derive(Debug, thiserror::Error)]
pub enum SessionAgentError {
    /// The Run's records could not be read or written.
    #[error("the run's records failed: {0:?}")]
    Records(rootcause::Report),
    /// The session could not be opened, prompted or read.
    #[error("the agent session failed: {0}")]
    Session(#[from] RoutineSessionError),
    /// The Agent's owner's quota for it is used up. Refused the Step's first
    /// prompt, the Agent could not start, and the next Agent of the Step's
    /// Role takes the Step; this fails it only when no Agent is left.
    /// Refused a later prompt, the Agent's work stays in its session, so no
    /// other Agent takes over: the Step's checks end there.
    #[error("the agent's quota ran out: {}", .0.code())]
    NoQuota(DenyReason),
    /// The agent definition lists no Agent for the Step's Role.
    #[error("no agent fills role {0}")]
    UnfilledRole(Role),
    /// The Step's session is of an Agent the agent definition no longer has.
    #[error("the agent definition has no agent {0}")]
    UnknownAgent(AgentName),
    /// The session did not open, or did not show it opened in time.
    #[error("the agent session did not open")]
    NotOpened,
    /// The prompt's delivery was unknown, and the session never showed it
    /// arrived.
    #[error("the agent session never showed its prompt arrived")]
    PromptUnsettled,
    /// The Step let go of its session, or it or its Run ended, while this
    /// attempt still ran: Temporal gave the Step to another attempt, or
    /// stopped it.
    #[error("the step no longer holds its agent session")]
    Released,
    /// The stop found no session, while a prepare sent lately may still
    /// open one: the stop is tried again until it opens or no prepare can.
    #[error("the agent session may still open")]
    MayStillOpen,
    /// The session's turn ended unsuccessfully.
    #[error("the agent's turn ended unsuccessfully: {0:?}")]
    TurnFailed(RoutineFailureReason),
    /// A later prompt was asked of a Step that claimed no session.
    #[error("the step has no agent session to go on in")]
    NoSession,
    /// The session's turn ended with nothing handed back.
    #[error("the agent's turn ended without handing back a result")]
    NothingHandedBack,
    /// Another attempt took the Step on to a later prompt while this one
    /// waited on its own, or before this one sent it: its hand back is not
    /// the prompt's, and a retry goes on from where the Step is now.
    #[error("the step went on to a later prompt of its session")]
    Superseded,
    /// A later prompt's turn was still going when the wait for it ended:
    /// this attempt's time, or the time its Step gave it. A retry waits on
    /// the same turn.
    #[error("the agent's turn on a later prompt was still going when the wait ended")]
    Overdue,
    /// The session a Step went on in after its question could not be
    /// resumed: its runtime stayed disconnected.
    #[error("the agent session could not be resumed")]
    SessionLost,
    /// The session worked past the Step's deadline.
    #[error("the agent did not finish within {0:?}")]
    TooSlow(Duration),
    /// The Run's owner stopped the Step from its Step Terminal.
    #[error("the run's owner stopped the step")]
    StoppedByOwner,
    /// The Step stayed paused past its work's time.
    #[error("the step stayed paused past its time")]
    PausedTooLong,
}

impl Retryable for SessionAgentError {
    /// A turn that ended without a result, or ran past its deadline, ends
    /// the same way when asked again, as does a Step its owner stopped, a
    /// refusal, a mismatch or a conflict from the session (only an outage
    /// passes on a retry), and a
    /// quota every Agent of the Role used up, or an agent definition that
    /// does not fill it. A
    /// prompt whose delivery stayed unknown is not tried again either:
    /// another attempt would send more.
    ///
    /// A prompt the Step went on from is retried: another attempt took the
    /// Step on to a later prompt, or raced this one to record the same one,
    /// and the next attempt reads the Step's rounds afresh and goes on from
    /// where the Step is, never from this attempt's stale view.
    ///
    /// The records are retried: the agent's reads and writes are keyed by
    /// the Step and violate no constraint in use, so a failure is the
    /// database's, and the Step's two hours bound one that is not. A session
    /// that never showed it opened is retried too: it follows only an
    /// outage's answer to the prepare, never a refusal, which ends the Step.
    fn retryable(&self) -> bool {
        match self {
            SessionAgentError::Records(_)
            | SessionAgentError::Overdue
            | SessionAgentError::Superseded
            | SessionAgentError::NotOpened
            | SessionAgentError::MayStillOpen => true,
            SessionAgentError::Session(error) => error.is_transient(),
            SessionAgentError::TurnFailed(_)
            | SessionAgentError::NoQuota(_)
            | SessionAgentError::UnfilledRole(_)
            | SessionAgentError::UnknownAgent(_)
            | SessionAgentError::NothingHandedBack
            | SessionAgentError::NoSession
            | SessionAgentError::PromptUnsettled
            | SessionAgentError::Released
            | SessionAgentError::SessionLost
            | SessionAgentError::TooSlow(_)
            | SessionAgentError::StoppedByOwner
            | SessionAgentError::PausedTooLong => false,
        }
    }
}

impl AgentFailure for SessionAgentError {
    fn out_of_quota(&self) -> bool {
        matches!(self, SessionAgentError::NoQuota(_))
    }
}

/// Runs each task in a session of an Agent of its Role, owned by the Run's
/// owner, and waits for the session's turn to end.
#[derive(Debug, Clone)]
pub struct SessionAgent<S, R> {
    sessions: S,
    repo: R,
    agents: AgentDefinition,
    pause_notices: Option<Notices>,
}

/// Where a paused Step's owner is told its time runs out.
#[derive(Clone)]
struct Notices(std::sync::Arc<dyn PauseNotices>);

impl std::fmt::Debug for Notices {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PauseNotices")
    }
}

impl<S: RoutineSessions, R: RunRepository> SessionAgent<S, R> {
    /// The Agents of `agents`, reached through `sessions`, with their
    /// sessions recorded in `repo`.
    pub fn new(sessions: S, repo: R, agents: AgentDefinition) -> Self {
        Self {
            sessions,
            repo,
            agents,
            pause_notices: None,
        }
    }

    /// Tell a paused Step's owner through `notices` when its time runs out.
    #[must_use]
    pub fn with_pause_notices(mut self, notices: impl PauseNotices) -> Self {
        self.pause_notices = Some(Notices(std::sync::Arc::new(notices)));
        self
    }

    /// The name of the Agent whose session `session` of a Step of `role`
    /// is: the one recorded, else, for a session claimed before Steps
    /// recorded their Agent, the first Agent of the Role.
    fn agent_name(
        &self,
        session: &StepSession,
        role: &Role,
    ) -> Result<AgentName, SessionAgentError> {
        match &session.agent {
            Some(agent) => Ok(agent.clone()),
            None => self
                .agents
                .for_role(role)
                .first()
                .cloned()
                .ok_or_else(|| SessionAgentError::UnfilledRole(role.clone())),
        }
    }

    /// The Agent named `name`.
    fn agent(&self, name: &AgentName) -> Result<&Agent, SessionAgentError> {
        self.agents
            .agent(name)
            .ok_or_else(|| SessionAgentError::UnknownAgent(name.clone()))
    }

    /// The bot whose harness runs the session `session` of a Step of
    /// `role`: the one recorded with it, so a stop or a release never
    /// depends on the agent definition still naming its Agent; else, for a
    /// session claimed before Steps recorded it, its Agent's.
    fn bot(&self, session: &StepSession, role: &Role) -> Result<BotId, SessionAgentError> {
        match session.bot {
            Some(bot) => Ok(BotId::new_from_uuid(bot)),
            None => Ok(self.agent(&self.agent_name(session, role)?)?.bot),
        }
    }

    /// A new session of the Agent `name`, its ids made now.
    fn new_session(&self, name: &AgentName) -> Result<StepSession, SessionAgentError> {
        Ok(StepSession {
            session_id: Uuid::now_v7(),
            action_id: Uuid::now_v7(),
            agent: Some(name.clone()),
            bot: Some(self.agent(name)?.bot.as_uuid()),
        })
    }

    /// The session the Step `key` works in now, when another attempt handed
    /// it on from `session`, and it still holds it; none when it still works
    /// in `session`. Fails once it let go of its session.
    async fn moved_on(
        &self,
        key: StepKey,
        session: &StepSession,
    ) -> Result<Option<StepSession>, SessionAgentError> {
        let current = self
            .repo
            .claimed_session(key)
            .await
            .map_err(Self::records)?
            .ok_or(SessionAgentError::Released)?;
        if current.session_id == session.session_id {
            return Ok(None);
        }
        self.still_holds(key, current.session_id).await?;
        Ok(Some(current))
    }

    /// What the agent handed back for the prompt `action_id` of the Step
    /// `key`, once its turn ended: never a later prompt's answer.
    async fn handed_back(
        &self,
        key: StepKey,
        action_id: Uuid,
    ) -> Result<AgentAnswer, SessionAgentError> {
        match self
            .repo
            .handed_back(key, action_id)
            .await
            .map_err(Self::records)?
        {
            HandedBack::Handed(handed) => Ok(AgentAnswer {
                outcome: handed.outcome,
                text: handed.output,
            }),
            HandedBack::Nothing => Err(SessionAgentError::NothingHandedBack),
            HandedBack::Superseded => Err(SessionAgentError::Superseded),
        }
    }

    fn records(error: impl std::error::Error + Send + Sync + 'static) -> SessionAgentError {
        SessionAgentError::Records(rootcause::Report::new(error).into_dynamic())
    }

    /// The first prompt of `owner`'s session `session`, run by `bot`.
    fn action(
        owner: &MacroUserIdStr<'static>,
        session: &StepSession,
        bot: BotId,
    ) -> RoutineSessionAction {
        RoutineSessionAction {
            owner: owner.clone(),
            bot_id: bot,
            session_id: AgentSessionId::new_from_uuid(session.session_id),
            action_id: AgentActionId::from_uuid(session.action_id),
        }
    }

    /// Open the Step's session, or find it open. A prepare that fails
    /// ambiguously (a timeout, a gateway's 504, a failed answer) may still
    /// have opened it, and prepare is never replayed after one, so the
    /// session's status settles it. A conflict means an earlier attempt
    /// opened it.
    async fn open(
        &self,
        task: &AgentTask,
        agent: &Agent,
        action: &RoutineSessionAction,
    ) -> Result<(), SessionAgentError> {
        let prepared = self
            .sessions
            .prepare(PrepareRoutineSession {
                selection: ValidateRoutineSession {
                    owner: task.owner.clone(),
                    bot_id: agent.bot,
                    model: agent.model.clone(),
                    mcp_servers: Vec::new(),
                },
                session_id: action.session_id,
                // The owner asked for the Run, and reads its Steps in it: a
                // background session of theirs, hidden, notifying nobody,
                // and deleted once the Step is done with it.
                source: SessionSource::User,
                visibility: SessionVisibility::Hidden,
                origin: TriggerOrigin::Requested {
                    by: task.owner.clone(),
                },
                billing: Default::default(),
                instructions: agent.instructions.clone(),
                repo_url: task.repo_url.clone(),
            })
            .await;
        match prepared {
            Ok(_) | Err(RoutineSessionError::Conflict) => Ok(()),
            Err(RoutineSessionError::OperationFailed | RoutineSessionError::RuntimeUnavailable) => {
                let deadline = tokio::time::Instant::now() + OPEN_SETTLE;
                let mut wait = FIRST_LOOK;
                loop {
                    if self.sessions.status(action.clone()).await.is_ok() {
                        return Ok(());
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(SessionAgentError::NotOpened);
                    }
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(LONGEST_WAIT);
                }
            }
            Err(error) => Err(refusal(error)),
        }
    }

    /// Prompt the session once. The session refuses a prompt it already
    /// has in its history or holds queued with a conflict, so a retried Step
    /// always asks, and only a session that took no prompt yet takes one. A
    /// prompt whose delivery is unknown may still land, so it is never sent
    /// again, as the routine sessions port says: the session's status
    /// settles it, and a session that shows no sign of it once the wait is
    /// over fails the Step for good.
    async fn prompt_once(
        &self,
        action: &RoutineSessionAction,
        prompted: Result<RoutinePromptAccepted, RoutineSessionError>,
    ) -> Result<(), SessionAgentError> {
        match prompted {
            Ok(_) | Err(RoutineSessionError::Conflict) => Ok(()),
            Err(RoutineSessionError::PromptDeliveryUnknown) => {
                if self.prompt_shows(action).await {
                    Ok(())
                } else {
                    Err(SessionAgentError::PromptUnsettled)
                }
            }
            Err(error) => Err(refusal(error)),
        }
    }

    /// Whether the session shows its first prompt within [`PROMPT_SETTLE`]:
    /// a turn that runs, waits on a person, or ended. A session awaiting its
    /// prompt, or whose runtime disconnected, shows nothing either way.
    async fn prompt_shows(&self, action: &RoutineSessionAction) -> bool {
        let deadline = tokio::time::Instant::now() + PROMPT_SETTLE;
        let mut wait = FIRST_LOOK;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(LONGEST_WAIT);
            let shows = matches!(
                self.sessions.status(action.clone()).await,
                Ok(RoutineActionStatus::Succeeded
                    | RoutineActionStatus::Failed(_)
                    | RoutineActionStatus::Pending(
                        RoutinePendingReason::Running
                            | RoutinePendingReason::Permission
                            | RoutinePendingReason::Elicitation
                    ))
            );
            if shows {
                return true;
            }
        }
        false
    }

    /// Delete the Step's session, which nothing reads once the Step is done
    /// with it. A delete that fails leaves a hidden session, so it is logged,
    /// not raised.
    async fn delete(&self, action: &RoutineSessionAction) {
        if let Err(error) = self.sessions.delete(action.clone()).await {
            tracing::warn!(?error, "a factory step's agent session was not deleted");
        }
    }

    /// Fails unless the Step `key` still holds the session `session_id`.
    async fn still_holds(&self, key: StepKey, session_id: Uuid) -> Result<(), SessionAgentError> {
        let holds = self
            .repo
            .holds_session(key, session_id)
            .await
            .map_err(Self::records)?;
        if holds {
            Ok(())
        } else {
            Err(SessionAgentError::Released)
        }
    }

    /// Ask the session to stop its turn. A stop that fails is logged: the
    /// caller watches the session to tell whether it still works.
    async fn cancel(&self, action: &RoutineSessionAction) {
        if let Err(error) = self.sessions.cancel(action.clone()).await {
            tracing::warn!(?error, "a factory step's agent session was not stopped");
        }
    }

    /// Wait for the turn of `action`, a prompt of the Step `key`'s session,
    /// to end, following the Run's owner's steering (`step_terminal`) until
    /// it closes: the turn of a note or a resume they sent after it, and no
    /// end while the Step is paused. A stop they asked fails the wait at
    /// once. The time the Step is paused does not count against `deadline`.
    async fn finished(
        &self,
        key: StepKey,
        action: &RoutineSessionAction,
        deadline: tokio::time::Instant,
    ) -> Result<(), SessionAgentError> {
        self.wait_turn(key, action, deadline, false).await
    }

    /// [`Self::finished`], for a session resumed after a wait (`resumed`):
    /// one that shows its runtime disconnected for [`RESUME_SETTLE`] in a row
    /// is lost.
    async fn wait_turn(
        &self,
        key: StepKey,
        action: &RoutineSessionAction,
        deadline: tokio::time::Instant,
        resumed: bool,
    ) -> Result<(), SessionAgentError> {
        let mut watched = action.clone();
        // The turns the wait followed, the first the Step's own: a prompt
        // of the owner's the session refused never arrives, and the wait
        // goes back to the one before it.
        let mut followed: Vec<AgentActionId> = vec![action.action_id];
        let mut warned = false;
        let mut deadline = deadline;
        let mut wait = FIRST_LOOK;
        let mut failed_reads = 0;
        let mut disconnected_since: Option<tokio::time::Instant> = None;
        loop {
            let steering = self.repo.steering(key).await.map_err(Self::records)?;
            if steering.stop_asked {
                // Its owner stopped the Step: the turn stops, and the Step
                // ends failed, its session stopped for good with it.
                self.cancel(&watched).await;
                return Err(SessionAgentError::StoppedByOwner);
            }
            // A Step paused near the end of its work's time tells its owner
            // once, and ends failed when the time runs out, saying why.
            if let Some(started_at) = steering.started_at.filter(|_| steering.paused) {
                let ends_by = paused_step_ends_by(started_at);
                let now = chrono::Utc::now();
                if now >= ends_by {
                    self.repo.pause_ran_out(key).await.map_err(Self::records)?;
                    self.cancel(&watched).await;
                    return Err(SessionAgentError::PausedTooLong);
                }
                if !warned && now >= ends_by - PAUSE_WARNING {
                    warned = true;
                    if let Some(notices) = &self.pause_notices {
                        notices.0.running_out(key, ends_by).await;
                    }
                }
            }
            // A read that fails now and then is the transport's; only a run
            // of them fails the Step.
            let ended = match self.sessions.status(watched.clone()).await {
                Ok(RoutineActionStatus::Succeeded) => Some(Ok(())),
                Ok(RoutineActionStatus::Failed(reason)) => {
                    Some(Err(SessionAgentError::TurnFailed(reason)))
                }
                Ok(RoutineActionStatus::Pending(RoutinePendingReason::Disconnected)) if resumed => {
                    failed_reads = 0;
                    let since = *disconnected_since.get_or_insert_with(tokio::time::Instant::now);
                    if since.elapsed() >= RESUME_SETTLE {
                        return Err(SessionAgentError::SessionLost);
                    }
                    None
                }
                // The owner's prompt the wait followed never reached the
                // session: it refused it, and the Step follows the turn
                // before it again.
                Ok(RoutineActionStatus::Pending(RoutinePendingReason::AwaitingPrompt))
                    if followed.len() > 1
                        && steering.prompt != Some(watched.action_id.as_uuid())
                        && steering.prompt.is_none_or(|prompt| {
                            followed.iter().any(|turn| turn.as_uuid() == prompt)
                        }) =>
                {
                    followed.pop();
                    while followed.len() > 1
                        && steering.prompt != followed.last().map(|turn| turn.as_uuid())
                    {
                        followed.pop();
                    }
                    watched.action_id = followed[followed.len() - 1];
                    continue;
                }
                Ok(RoutineActionStatus::Pending(pending)) => {
                    failed_reads = 0;
                    disconnected_since = None;
                    // A turn that still runs while the Step is paused (its
                    // stop was lost, or a note queued before the pause
                    // started) is stopped again.
                    if steering.paused && pending == RoutinePendingReason::Running {
                        self.cancel(&watched).await;
                    }
                    None
                }
                Err(error) => {
                    failed_reads += 1;
                    if failed_reads >= MAX_FAILED_READS {
                        return Err(error.into());
                    }
                    None
                }
            };

            if let Some(ended) = ended {
                match steering.prompt {
                    // The owner sent a note or a resume after this turn: the
                    // Step goes on with its turn.
                    Some(prompt) if prompt != watched.action_id.as_uuid() => {
                        watched.action_id = AgentActionId::from_uuid(prompt);
                        followed.push(watched.action_id);
                        wait = FIRST_LOOK;
                        continue;
                    }
                    // Paused: the turn the pause stopped is not the Step's
                    // end; the resume's turn is.
                    _ if steering.paused => {}
                    followed => {
                        if self
                            .repo
                            .close_steering(key, followed)
                            .await
                            .map_err(Self::records)?
                        {
                            return ended;
                        }
                        // Steered since the read: read it again.
                        continue;
                    }
                }
            }

            if steering.paused {
                deadline += wait;
            } else if tokio::time::Instant::now() >= deadline {
                return Err(SessionAgentError::TooSlow(SESSION_DEADLINE));
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(LONGEST_WAIT);
        }
    }
}

impl<S: RoutineSessions, R: RunRepository> SessionAgent<S, R> {
    /// Open the Step's `session` and prompt it, as the session's Agent, and
    /// the action to wait on. An Agent whose quota is used up refuses either
    /// before any work starts: its session, if it opened, is deleted, and
    /// the error says so.
    async fn start(
        &self,
        task: &AgentTask,
        session: &StepSession,
        agent_name: &AgentName,
    ) -> Result<RoutineSessionAction, SessionAgentError> {
        let agent = self.agent(agent_name)?;
        let action = Self::action(&task.owner, session, self.bot(session, &task.role)?);

        // A deleted session is gone: preparing it again would open a fresh
        // one under its id, and prompting it would start the work over. An
        // attempt Temporal gave up on may still run here after the Step let
        // go of its session, or handed it to another Agent, so it checks
        // before each that the Step still works in this session.
        //
        // The prepare is recorded at the database's time before it is sent,
        // so a stop can tell a session that may still open from one that
        // never will. A Step that let go of its session, or that ended, or
        // whose Run ended, sends no prepare.
        let Some(prepared_at) = self
            .repo
            .record_prepare(task.key, session.session_id)
            .await
            .map_err(Self::records)?
        else {
            return Err(SessionAgentError::Released);
        };
        if let Err(error) = self.open(task, agent, &action).await {
            // Refused outright, or never shown open within the settle: this
            // prepare is not in flight, so a stop need not wait for it. A
            // later attempt's prepare keeps its record.
            self.repo
                .forget_prepare(task.key, prepared_at)
                .await
                .map_err(Self::records)?;
            return Err(error);
        }
        // The session just opened is this attempt's to delete when the Step
        // let go of it meanwhile: nothing else will.
        if let Err(released) = self.still_holds(task.key, session.session_id).await {
            self.delete(&action).await;
            return Err(released);
        }
        // A session that opened is never written off by a stop.
        self.repo
            .record_open(task.key, session.session_id)
            .await
            .map_err(Self::records)?;
        let prompted = self
            .sessions
            .prompt(PromptRoutineSession {
                action: action.clone(),
                prompt: prompt(&task.prompt, &task.outcomes),
                context: None,
                out_of_sight: true,
            })
            .await;
        if let Err(error) = self.prompt_once(&action, prompted).await {
            // A refused prompt started no work: the session is no use to
            // the next Agent, and the Step stops pointing at it once handed
            // on, so it is deleted here.
            if matches!(error, SessionAgentError::NoQuota(_)) {
                self.delete(&action).await;
            }
            return Err(error);
        }
        Ok(action)
    }
}

impl<S: RoutineSessions, R: RunRepository> SessionAgent<S, R> {
    /// Do the Step `task`, which goes on from the question of the Step at
    /// `asked`, in that Step's session: the agent has the Step's
    /// instructions and its work so far there. The Step claims the session
    /// under a prompt of its own, so a retry goes on with that prompt, and
    /// the session's hand back reaches this Step, the one that runs. No
    /// other Agent takes it over, whatever the session answers: the work is
    /// in this one.
    ///
    /// The harness may have let the session go while the Run waited on its
    /// owner: it stops an idle sandbox, then restores the session on the
    /// next prompt. A session that cannot be resumed ([`Self::lost`]) is
    /// deleted, and the Step goes on in a new session of its Role's first
    /// Agent, prompted with the whole Step afresh (`AgentTask::afresh`),
    /// the owner's answers in it.
    async fn go_on(
        &self,
        task: &AgentTask,
        asked: u16,
        deadline: tokio::time::Instant,
    ) -> Result<AgentAnswer, SessionAgentError> {
        let asking = StepKey {
            run_id: task.key.run_id,
            seq: asked,
        };
        let asked_in = self
            .repo
            .claimed_session(asking)
            .await
            .map_err(Self::records)?
            .ok_or(SessionAgentError::NoSession)?;
        let session = self
            .repo
            .claim_session(
                task.key,
                StepSession {
                    action_id: Uuid::now_v7(),
                    ..asked_in.clone()
                },
                &task.role,
                &task.outcomes,
            )
            .await
            .map_err(Self::records)?
            .session;
        // An earlier attempt found the session lost and went on afresh.
        if session.session_id != asked_in.session_id {
            return self.afresh(task, session, deadline).await;
        }

        let resumed = self.resume(task, &session, deadline).await;
        match resumed {
            Err(error) if lost(&error) && task.afresh.is_some() => {
                tracing::warn!(
                    ?error,
                    "a factory step's agent session could not be resumed after its question: the step goes on afresh"
                );
            }
            resumed => return resumed,
        }

        // The lost session is deleted first, so a prompt of it that lands
        // late never works beside the new one.
        let action = Self::action(&task.owner, &session, self.bot(&session, &task.role)?);
        self.delete(&action).await;
        let agent = self.agent_name(&session, &task.role)?;
        let first = self
            .agents
            .for_role(&task.role)
            .first()
            .cloned()
            .ok_or_else(|| SessionAgentError::UnfilledRole(task.role.clone()))?;
        let fresh = self.new_session(&first)?;
        let passed = PassedOver {
            agent,
            reason: SESSION_LOST.to_string(),
        };
        let recorded = self
            .repo
            .pass_session(
                task.key,
                session.session_id,
                &task.role,
                &passed,
                Some(&fresh),
            )
            .await
            .map_err(Self::records)?;
        let session = if recorded {
            fresh
        } else {
            // Another attempt went on afresh already: go on in its session.
            match self.moved_on(task.key, &session).await? {
                Some(current) => current,
                None => return Err(SessionAgentError::Released),
            }
        };
        self.afresh(task, session, deadline).await
    }

    /// Prompt the asking Step's `session`, which the Step `task` claimed,
    /// with the owner's answer, and wait for the turn to end.
    async fn resume(
        &self,
        task: &AgentTask,
        session: &StepSession,
        deadline: tokio::time::Instant,
    ) -> Result<AgentAnswer, SessionAgentError> {
        self.still_holds(task.key, session.session_id).await?;
        // The session opened for the asking Step, so a stop of this one
        // deletes it rather than write it off.
        self.repo
            .record_open(task.key, session.session_id)
            .await
            .map_err(Self::records)?;
        let action = Self::action(&task.owner, session, self.bot(session, &task.role)?);
        let prompted = self
            .sessions
            .follow_up(PromptRoutineSession {
                action: action.clone(),
                prompt: prompt(&task.prompt, &task.outcomes),
                context: None,
                out_of_sight: true,
            })
            .await;
        self.prompt_once(&action, prompted).await?;
        self.wait_turn(task.key, &action, deadline, true).await?;
        self.handed_back(task.key, action.action_id.as_uuid()).await
    }

    /// Do the Step `task` afresh in `session`, a new session it claimed in
    /// place of the one it could not resume, prompted with the whole Step.
    async fn afresh(
        &self,
        task: &AgentTask,
        session: StepSession,
        deadline: tokio::time::Instant,
    ) -> Result<AgentAnswer, SessionAgentError> {
        let prompt = task.afresh.clone().ok_or(SessionAgentError::NoSession)?;
        let task = AgentTask {
            prompt,
            resumes: None,
            afresh: None,
            ..task.clone()
        };
        self.answer_in(&task, session, deadline).await
    }

    /// Do the Step `task` in `session`, the one it claimed: open it and
    /// prompt it as its Agent, handing the Step to the next Agent of its
    /// Role while one cannot start for lack of quota, and wait for the
    /// hand back.
    async fn answer_in(
        &self,
        task: &AgentTask,
        session: StepSession,
        deadline: tokio::time::Instant,
    ) -> Result<AgentAnswer, SessionAgentError> {
        let agents = self.agents.for_role(&task.role);
        let mut session = session;
        // An Agent that cannot start for lack of quota hands the Step to the
        // next Agent of its Role, in a session of its own, until one starts
        // or none is left; the Step records each Agent passed over, the last
        // one too.
        let action = loop {
            let agent = self.agent_name(&session, &task.role)?;
            let reason = match self.start(task, &session, &agent).await {
                Ok(action) => break action,
                Err(SessionAgentError::NoQuota(reason)) => reason,
                // An overlapping attempt handed the Step on while this one
                // opened its session: go on in the session it works in now.
                Err(SessionAgentError::Released) => {
                    match self.moved_on(task.key, &session).await? {
                        Some(current) => {
                            session = current;
                            continue;
                        }
                        None => return Err(SessionAgentError::Released),
                    }
                }
                Err(error) => return Err(error),
            };
            // The Agent after this one in the Role's list, or its first
            // when the definition no longer lists this one for the Role.
            let next = agents
                .iter()
                .position(|listed| *listed == agent)
                .map_or(0, |place| place + 1);
            let to = agents
                .get(next)
                .map(|name| self.new_session(name))
                .transpose()?;
            let passed = PassedOver {
                agent: agent.clone(),
                reason: reason.code().to_string(),
            };
            let recorded = self
                .repo
                .pass_session(
                    task.key,
                    session.session_id,
                    &task.role,
                    &passed,
                    to.as_ref(),
                )
                .await
                .map_err(Self::records)?;
            let Some(to) = to else {
                return Err(SessionAgentError::NoQuota(reason));
            };
            if recorded {
                tracing::info!(
                    from = %agent,
                    reason = reason.code(),
                    "a factory step's agent could not start: the next agent takes it"
                );
                session = to;
                continue;
            }

            // Another attempt handed the Step on already, or it let go of
            // its session: go on in the session it works in now, while it
            // holds it.
            match self.moved_on(task.key, &session).await? {
                Some(current) => session = current,
                None => return Err(SessionAgentError::NoQuota(reason)),
            }
        };
        self.finished(task.key, &action, deadline).await?;

        // The session stays until the Step's end is recorded: a retry of an
        // attempt whose answer Temporal lost goes on in it.
        self.handed_back(task.key, action.action_id.as_uuid()).await
    }
}

impl<S: RoutineSessions, R: RunRepository> StepAgent for SessionAgent<S, R> {
    type Err = SessionAgentError;

    #[tracing::instrument(err, skip_all, fields(run_id = %task.key.run_id, seq = task.key.seq, step = %task.step, role = %task.role))]
    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, SessionAgentError> {
        // Opening and prompting count against the deadline too: the sooner
        // of SESSION_DEADLINE from this call and the time the Step gave.
        let left = task.answer_by.map_or(SESSION_DEADLINE, |answer_by| {
            until(answer_by).min(SESSION_DEADLINE)
        });
        let deadline = tokio::time::Instant::now() + left;
        if let Some(asked) = task.resumes {
            return self.go_on(&task, asked, deadline).await;
        }
        let first = self
            .agents
            .for_role(&task.role)
            .first()
            .ok_or_else(|| SessionAgentError::UnfilledRole(task.role.clone()))?;
        // A retried Step goes on in the session its first attempt claimed,
        // or the one a later Agent took over, so the work is never started
        // twice.
        let session = self
            .repo
            .claim_session(
                task.key,
                self.new_session(first)?,
                &task.role,
                &task.outcomes,
            )
            .await
            .map_err(Self::records)?
            .session;

        self.answer_in(&task, session, deadline).await
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %turn.key.run_id, seq = turn.key.seq))]
    async fn follow_up(&self, turn: AgentTurn) -> Result<AgentAnswer, SessionAgentError> {
        // The sooner of SESSION_DEADLINE from this call and the time the
        // Step gave. Past either, the wait ends as `Overdue`, which a retry
        // goes on from, waiting on the same turn.
        let deadline = tokio::time::Instant::now() + until(turn.answer_by).min(SESSION_DEADLINE);
        let session = self
            .repo
            .claimed_session(turn.key)
            .await
            .map_err(Self::records)?
            .ok_or(SessionAgentError::NoSession)?;
        // The session the Step works in now, of the Agent that took it when
        // others were passed over, and run by the bot recorded with it.
        let action = RoutineSessionAction {
            action_id: AgentActionId::from_uuid(turn.action_id),
            ..Self::action(&turn.owner, &session, self.bot(&session, &turn.role)?)
        };

        // Recorded before the prompt is sent: the agent's hand back from now
        // on is this prompt's, and a stop watches this prompt. A retry
        // records it again, which keeps what was handed back since. Both
        // hold only while the Step works in this session.
        self.still_holds(turn.key, session.session_id).await?;
        let began = self
            .repo
            .begin_turn(
                turn.key,
                session.session_id,
                turn.action_id,
                turn.place,
                &turn.outcomes,
            )
            .await
            .map_err(Self::records)?;
        // Left alone: the Step let go of this session, or another attempt
        // took it on to a later prompt, which a retry goes on from.
        if !began {
            self.still_holds(turn.key, session.session_id).await?;
            return Err(SessionAgentError::Superseded);
        }
        let prompted = self
            .sessions
            .follow_up(PromptRoutineSession {
                action: action.clone(),
                prompt: prompt(&turn.prompt, &turn.outcomes),
                context: None,
                out_of_sight: true,
            })
            .await;
        self.prompt_once(&action, prompted).await?;
        match self.finished(turn.key, &action, deadline).await {
            Err(SessionAgentError::TooSlow(_)) => return Err(SessionAgentError::Overdue),
            finished => finished?,
        }

        self.handed_back(turn.key, turn.action_id).await
    }

    /// Runs in the session the Step works in now, through its bot, while
    /// the Step still holds it. The command is named by an id of its own,
    /// which no prompt takes.
    #[tracing::instrument(err, skip_all, fields(run_id = %command.key.run_id, seq = command.key.seq))]
    async fn run_check(
        &self,
        command: AgentCommand,
    ) -> Result<Option<SessionCommandOutput>, SessionAgentError> {
        let session = self
            .repo
            .claimed_session(command.key)
            .await
            .map_err(Self::records)?
            .ok_or(SessionAgentError::NoSession)?;
        self.still_holds(command.key, session.session_id).await?;
        let action = RoutineSessionAction {
            action_id: AgentActionId::from_uuid(Uuid::now_v7()),
            ..Self::action(&command.owner, &session, self.bot(&session, &command.role)?)
        };
        match self
            .sessions
            .exec(ExecRoutineSession {
                action,
                command: command.command,
            })
            .await
        {
            Ok(output) => Ok(Some(output)),
            Err(RoutineSessionError::CommandUnsupported) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %key.run_id, seq = key.seq))]
    async fn stop(
        &self,
        key: StepKey,
        role: &Role,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<(), SessionAgentError> {
        // Recorded first, so no attempt prepares or opens the session again
        // once it is deleted, and what the attempts recorded before is all
        // there will be.
        self.repo
            .release_session(key)
            .await
            .map_err(Self::records)?;
        let stopping = self
            .repo
            .stopping_session(key, PREPARE_LANDS_WITHIN)
            .await
            .map_err(Self::records)?;
        let Some(stopping) = stopping else {
            return Ok(());
        };
        // A session no attempt saw open, and no prepare sent lately may
        // still open, is not there and never will be. Any other session may
        // be working, or about to open, whatever a read or a delete answers:
        // the client's catch-all error is as likely a timeout as a missing
        // session, and a delete answers a missing session as deleted.
        let written_off = !stopping.opened && !stopping.may_still_open;

        // A prompt already on its way can land after the first stop and
        // start the turn again, so the stop is sent again while the session
        // still works, until its turn has ended or the watch is over. One
        // whose prepare is still in flight, or a harness that fails a while,
        // answers neither the stop nor a read, yet may open or work on, so
        // the watch rides that out.
        let action = Self::action(owner, &stopping.session, self.bot(&stopping.session, role)?);
        self.cancel(&action).await;
        let mut seen = false;
        let deadline = tokio::time::Instant::now() + STOP_WATCH;
        let mut wait = FIRST_LOOK;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(LONGEST_WAIT);
            match self.sessions.status(action.clone()).await {
                Ok(RoutineActionStatus::Succeeded | RoutineActionStatus::Failed(_)) => {
                    seen = true;
                    break;
                }
                Ok(RoutineActionStatus::Pending(RoutinePendingReason::AwaitingPrompt)) => {
                    seen = true;
                }
                Ok(RoutineActionStatus::Pending(_)) => {
                    seen = true;
                    self.cancel(&action).await;
                }
                Err(_) if !seen && written_off => break,
                Err(_) => {}
            }
        }
        // A delete that fails fails the stop, which Temporal tries again,
        // unless the session was written off: then there is nothing to
        // delete. A delete answers a missing session as deleted, yet one no
        // attempt saw open, whose prepare was sent lately, may open after it:
        // the stop is tried again until it opens and is deleted, or no
        // prepare can open it any more.
        match self.sessions.delete(action).await {
            Ok(()) if !seen && !stopping.opened && stopping.may_still_open => {
                Err(SessionAgentError::MayStillOpen)
            }
            Ok(()) => Ok(()),
            Err(_) if !seen && written_off => {
                tracing::info!("a stopped factory step never opened its agent session");
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Acts on the session the Step works in, through the bot recorded
    /// with it. A prompt goes as a later prompt of the session, under the
    /// id the Step keeps: one already taken, or whose delivery is unknown,
    /// is taken, since it may land.
    #[tracing::instrument(err, skip_all, fields(run_id = %key.run_id, seq = key.seq))]
    async fn steer(
        &self,
        key: StepKey,
        role: &Role,
        owner: &MacroUserIdStr<'static>,
        session: &StepSession,
        steer: SessionSteer,
    ) -> Result<(), SessionAgentError> {
        let action = Self::action(owner, session, self.bot(session, role)?);
        match steer {
            SessionSteer::Interrupt => {
                self.sessions.cancel(action).await?;
                Ok(())
            }
            SessionSteer::Prompt { prompt_id, prompt } => {
                let prompted = self
                    .sessions
                    .follow_up(PromptRoutineSession {
                        action: RoutineSessionAction {
                            action_id: AgentActionId::from_uuid(prompt_id),
                            ..action
                        },
                        prompt,
                        context: None,
                        out_of_sight: true,
                    })
                    .await;
                match prompted {
                    Ok(_)
                    | Err(
                        RoutineSessionError::Conflict | RoutineSessionError::PromptDeliveryUnknown,
                    ) => Ok(()),
                    Err(error) => Err(error.into()),
                }
            }
        }
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %key.run_id, seq = key.seq))]
    async fn release(
        &self,
        key: StepKey,
        role: &Role,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<(), SessionAgentError> {
        let claimed = self
            .repo
            .claimed_session(key)
            .await
            .map_err(Self::records)?;
        if let Some(session) = claimed {
            let bot = self.bot(&session, role)?;
            self.repo
                .release_session(key)
                .await
                .map_err(Self::records)?;
            self.delete(&Self::action(owner, &session, bot)).await;
        }
        Ok(())
    }
}

/// Whether a session a Step went on in after its question is lost: its
/// runtime stayed disconnected, it never showed the prompt, or it holds no
/// history to go on from.
fn lost(error: &SessionAgentError) -> bool {
    matches!(
        error,
        SessionAgentError::SessionLost
            | SessionAgentError::PromptUnsettled
            | SessionAgentError::Session(RoutineSessionError::InvalidCommand)
    )
}

/// What a session's refusal means for the Step: an Agent out of quota
/// passes the Step on; anything else is the session's failure.
fn refusal(error: RoutineSessionError) -> SessionAgentError {
    match error {
        RoutineSessionError::Admission(AiAdmissionError::Denied(reason)) => {
            SessionAgentError::NoQuota(reason)
        }
        error => SessionAgentError::Session(error),
    }
}

/// How long from now until `at`; nothing once it passed.
fn until(at: chrono::DateTime<chrono::Utc>) -> Duration {
    (at - chrono::Utc::now()).to_std().unwrap_or(Duration::ZERO)
}

/// `text`, then how to hand back one of `outcomes`.
fn prompt(text: &str, outcomes: &[Outcome]) -> String {
    let rendered = outcomes
        .iter()
        .map(|outcome| format!("`{outcome}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let first = outcomes.first().map_or("done", Outcome::as_str);
    let hand_back = HAND_BACK
        .replace("OUTCOMES", &rendered)
        .replace("OUTCOME", first);
    format!("{text}{hand_back}")
}
