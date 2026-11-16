//! A [`StepAgent`] that does a Step's work in a Macro agent session, run as
//! the Run's owner through the routine sessions capability, and reads the
//! result the agent hands back from it.

#[cfg(test)]
mod test;

use std::time::Duration;

use agent_runtime_protocol::domain::action::AgentActionId;
use agent_session::domain::model::{AgentSessionId, SessionSource, SessionVisibility};
use agent_session::domain::routines::{
    PrepareRoutineSession, PromptRoutineSession, RoutineActionStatus, RoutineFailureReason,
    RoutinePendingReason, RoutineSessionAction, RoutineSessionError, RoutineSessions,
    ValidateRoutineSession,
};
use bot_id::BotId;
use factory_records::domain::models::{Outcome, StepKey, StepSession};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use trigger_context::TriggerOrigin;
use uuid::Uuid;

use crate::domain::models::{AgentAnswer, AgentTask};
use crate::domain::ports::{Retryable, StepAgent};

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
    /// The session's turn ended with nothing handed back.
    #[error("the agent's turn ended without handing back a result")]
    NothingHandedBack,
    /// The session worked past the Step's deadline.
    #[error("the agent did not finish within {0:?}")]
    TooSlow(Duration),
}

impl Retryable for SessionAgentError {
    /// A turn that ended without a result, or ran past its deadline, ends
    /// the same way when asked again, as does a refusal, a mismatch or a
    /// conflict from the session (only an outage passes on a retry). A
    /// prompt whose delivery stayed unknown is not tried again either:
    /// another attempt would send more.
    ///
    /// The records are retried: the agent's reads and writes are keyed by
    /// the Step and violate no constraint in use, so a failure is the
    /// database's, and the Step's two hours bound one that is not. A session
    /// that never showed it opened is retried too: it follows only an
    /// outage's answer to the prepare, never a refusal, which ends the Step.
    fn retryable(&self) -> bool {
        match self {
            SessionAgentError::Records(_)
            | SessionAgentError::NotOpened
            | SessionAgentError::MayStillOpen => true,
            SessionAgentError::Session(error) => error.is_transient(),
            SessionAgentError::TurnFailed(_)
            | SessionAgentError::NothingHandedBack
            | SessionAgentError::PromptUnsettled
            | SessionAgentError::Released
            | SessionAgentError::TooSlow(_) => false,
        }
    }
}

/// Runs each task in a session of the agent `bot_id`, owned by the Run's
/// owner, and waits for the session's turn to end.
#[derive(Debug, Clone)]
pub struct SessionAgent<S, R> {
    sessions: S,
    repo: R,
    bot_id: BotId,
}

impl<S: RoutineSessions, R: RunRepository> SessionAgent<S, R> {
    /// The agent `bot_id`, reached through `sessions`, with its sessions
    /// recorded in `repo`.
    pub fn new(sessions: S, repo: R, bot_id: BotId) -> Self {
        Self {
            sessions,
            repo,
            bot_id,
        }
    }

    fn records(error: impl std::error::Error + Send + Sync + 'static) -> SessionAgentError {
        SessionAgentError::Records(rootcause::Report::new(error).into_dynamic())
    }

    /// The first prompt of `owner`'s session `session`.
    fn action(
        &self,
        owner: &MacroUserIdStr<'static>,
        session: StepSession,
    ) -> RoutineSessionAction {
        RoutineSessionAction {
            owner: owner.clone(),
            bot_id: self.bot_id,
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
        action: &RoutineSessionAction,
    ) -> Result<(), SessionAgentError> {
        let prepared = self
            .sessions
            .prepare(PrepareRoutineSession {
                selection: ValidateRoutineSession {
                    owner: task.owner.clone(),
                    bot_id: self.bot_id,
                    model: None,
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
                instructions: None,
                repo_url: None,
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
            Err(error) => Err(error.into()),
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
        task: &AgentTask,
        action: &RoutineSessionAction,
    ) -> Result<(), SessionAgentError> {
        let prompted = self
            .sessions
            .prompt(PromptRoutineSession {
                action: action.clone(),
                prompt: prompt(task),
                context: None,
            })
            .await;
        match prompted {
            Ok(_) | Err(RoutineSessionError::Conflict) => Ok(()),
            Err(RoutineSessionError::PromptDeliveryUnknown) => {
                if self.prompt_shows(action).await {
                    Ok(())
                } else {
                    Err(SessionAgentError::PromptUnsettled)
                }
            }
            Err(error) => Err(error.into()),
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

    /// Fails unless the Step `key` still holds its session.
    async fn still_holds(&self, key: StepKey) -> Result<(), SessionAgentError> {
        if self.repo.holds_session(key).await.map_err(Self::records)? {
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

    /// Wait for the first prompt's turn to end, until `deadline`.
    async fn finished(
        &self,
        action: &RoutineSessionAction,
        deadline: tokio::time::Instant,
    ) -> Result<(), SessionAgentError> {
        let mut wait = FIRST_LOOK;
        let mut failed_reads = 0;
        loop {
            // A read that fails now and then is the transport's; only a run
            // of them fails the Step.
            match self.sessions.status(action.clone()).await {
                Ok(RoutineActionStatus::Succeeded) => return Ok(()),
                Ok(RoutineActionStatus::Failed(reason)) => {
                    return Err(SessionAgentError::TurnFailed(reason));
                }
                Ok(RoutineActionStatus::Pending(_)) => failed_reads = 0,
                Err(error) => {
                    failed_reads += 1;
                    if failed_reads >= MAX_FAILED_READS {
                        return Err(error.into());
                    }
                }
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(SessionAgentError::TooSlow(SESSION_DEADLINE));
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(LONGEST_WAIT);
        }
    }
}

impl<S: RoutineSessions, R: RunRepository> StepAgent for SessionAgent<S, R> {
    type Err = SessionAgentError;

    #[tracing::instrument(err, skip_all, fields(run_id = %task.key.run_id, seq = task.key.seq, step = %task.step))]
    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, SessionAgentError> {
        // Opening and prompting count against the deadline too.
        let deadline = tokio::time::Instant::now() + SESSION_DEADLINE;
        // A retried Step goes on in the session its first attempt claimed,
        // so the work is never started twice.
        let claimed = self
            .repo
            .claim_session(
                task.key,
                StepSession {
                    session_id: Uuid::now_v7(),
                    action_id: Uuid::now_v7(),
                },
                &task.outcomes,
            )
            .await
            .map_err(Self::records)?;
        let action = self.action(&task.owner, claimed.session);

        // A deleted session is gone: preparing it again would open a fresh
        // one under its id, and prompting it would start the work over. An
        // attempt Temporal gave up on may still run here after the Step let
        // go of its session, so it checks before each.
        //
        // The prepare is recorded at the database's time before it is sent,
        // so a stop can tell a session that may still open from one that
        // never will. A Step that let go of its session, or that ended, or
        // whose Run ended, sends no prepare.
        let Some(prepared_at) = self
            .repo
            .record_prepare(task.key)
            .await
            .map_err(Self::records)?
        else {
            return Err(SessionAgentError::Released);
        };
        if let Err(error) = self.open(&task, &action).await {
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
        if let Err(released) = self.still_holds(task.key).await {
            self.delete(&action).await;
            return Err(released);
        }
        // A session that opened is never written off by a stop.
        self.repo
            .record_open(task.key)
            .await
            .map_err(Self::records)?;
        self.prompt_once(&task, &action).await?;
        self.finished(&action, deadline).await?;

        // The session stays until the Step's end is recorded: a retry of an
        // attempt whose answer Temporal lost goes on in it.
        let handed = self
            .repo
            .handed_back(task.key)
            .await
            .map_err(Self::records)?
            .ok_or(SessionAgentError::NothingHandedBack)?;
        Ok(AgentAnswer {
            outcome: handed.outcome,
            text: handed.output,
        })
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %key.run_id, seq = key.seq))]
    async fn stop(
        &self,
        key: StepKey,
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
        let action = self.action(owner, stopping.session);
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

    #[tracing::instrument(err, skip_all, fields(run_id = %key.run_id, seq = key.seq))]
    async fn release(
        &self,
        key: StepKey,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<(), SessionAgentError> {
        let claimed = self
            .repo
            .claimed_session(key)
            .await
            .map_err(Self::records)?;
        if let Some(session) = claimed {
            self.repo
                .release_session(key)
                .await
                .map_err(Self::records)?;
            self.delete(&self.action(owner, session)).await;
        }
        Ok(())
    }
}

/// The task's prompt, then how to hand back one of its outcomes.
fn prompt(task: &AgentTask) -> String {
    let outcomes = task
        .outcomes
        .iter()
        .map(|outcome| format!("`{outcome}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let first = task.outcomes.first().map_or("done", Outcome::as_str);
    let hand_back = HAND_BACK
        .replace("OUTCOMES", &outcomes)
        .replace("OUTCOME", first);
    format!("{}{hand_back}", task.prompt)
}
