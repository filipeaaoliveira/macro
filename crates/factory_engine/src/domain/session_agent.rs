//! A [`StepAgent`] that does a Step's work in a Macro agent session, run as
//! the Run's owner through the routine sessions capability, and reads the
//! result the agent hands back from it.

#[cfg(test)]
mod test;

use std::time::Duration;

use agent_runtime_protocol::domain::action::AgentActionId;
use agent_session::domain::model::{AgentSessionId, SessionSource};
use agent_session::domain::routines::{
    PrepareRoutineSession, PromptRoutineSession, RoutineActionStatus, RoutineFailureReason,
    RoutinePendingReason, RoutineSessionAction, RoutineSessionError, RoutineSessions,
    ValidateRoutineSession,
};
use bot_id::BotId;
use factory_records::domain::models::{Outcome, StepKey, StepSession};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{AgentAnswer, AgentTask};
use crate::domain::ports::StepAgent;

/// What the agent is told about handing its result back.
const HAND_BACK: &str = include_str!("session_agent/hand_back.md");

/// How long to wait before the first look at the session, and the longest
/// wait between two looks.
const FIRST_LOOK: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(10);

/// How long an ambiguous prepare is given to show its session open: past the
/// longest a machine's agent takes to open one.
const OPEN_SETTLE: Duration = Duration::from_secs(200);

/// How long a prompt whose delivery is unknown is given to show in the
/// session, and how many prompts the Step sends at most.
const PROMPT_SETTLE: Duration = Duration::from_secs(60);
const PROMPT_TRIES: u8 = 2;

/// How long a stopped Step's session is watched, to stop it again if a
/// prompt still on its way started it.
const STOP_WATCH: Duration = Duration::from_secs(60);

/// How many reads of the session's status in a row may fail before the
/// Step gives up on it.
const MAX_FAILED_READS: u8 = 5;

/// How long a Step's session may work before the Step gives up on it, short
/// of the Step's own timeout so the failure says why.
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
                source: SessionSource::User,
                visibility: Default::default(),
                origin: Default::default(),
                billing: Default::default(),
                instructions: None,
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

    /// Prompt the session once. A prompt whose delivery is unknown is never
    /// sent again on that word alone: the session's status settles it. Only
    /// a session that still shows no sign of the prompt (`AwaitingPrompt`)
    /// once the wait is over is prompted again, and the session refuses a
    /// prompt it already has or holds queued with a conflict.
    async fn prompt_once(
        &self,
        task: &AgentTask,
        action: &RoutineSessionAction,
    ) -> Result<(), SessionAgentError> {
        for _ in 0..PROMPT_TRIES {
            // A retried Step whose session already has the prompt goes on.
            if self.has_prompt(action).await {
                return Ok(());
            }

            let prompted = self
                .sessions
                .prompt(PromptRoutineSession {
                    action: action.clone(),
                    prompt: prompt(task),
                    context: None,
                })
                .await;
            match prompted {
                Ok(_) | Err(RoutineSessionError::Conflict) => return Ok(()),
                Err(RoutineSessionError::PromptDeliveryUnknown) => {
                    let deadline = tokio::time::Instant::now() + PROMPT_SETTLE;
                    let mut wait = FIRST_LOOK;
                    while tokio::time::Instant::now() < deadline {
                        tokio::time::sleep(wait).await;
                        wait = (wait * 2).min(LONGEST_WAIT);
                        if self.has_prompt(action).await {
                            return Ok(());
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(RoutineSessionError::PromptDeliveryUnknown.into())
    }

    /// Whether the session's status shows its first prompt: anything but
    /// `AwaitingPrompt`, which says the prompt is not in its history.
    async fn has_prompt(&self, action: &RoutineSessionAction) -> bool {
        !matches!(
            self.sessions.status(action.clone()).await,
            Ok(RoutineActionStatus::Pending(
                RoutinePendingReason::AwaitingPrompt
            )) | Err(_)
        )
    }

    /// Wait for the first prompt's turn to end.
    async fn finished(&self, action: &RoutineSessionAction) -> Result<(), SessionAgentError> {
        let started = tokio::time::Instant::now();
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

            if started.elapsed() >= SESSION_DEADLINE {
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

        self.open(&task, &action).await?;
        self.prompt_once(&task, &action).await?;
        self.finished(&action).await?;

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
        let claimed = self
            .repo
            .claimed_session(key)
            .await
            .map_err(Self::records)?;
        let Some(session) = claimed else {
            return Ok(());
        };

        // A prompt already on its way can land after the first stop and
        // start the turn again, so the stop is sent again while the session
        // still works, until its turn has ended or the watch is over.
        let action = self.action(owner, session);
        self.sessions.cancel(action.clone()).await?;
        let deadline = tokio::time::Instant::now() + STOP_WATCH;
        let mut wait = FIRST_LOOK;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(LONGEST_WAIT);
            match self.sessions.status(action.clone()).await {
                Ok(RoutineActionStatus::Succeeded | RoutineActionStatus::Failed(_)) => {
                    return Ok(());
                }
                Ok(RoutineActionStatus::Pending(RoutinePendingReason::AwaitingPrompt))
                | Err(_) => {}
                Ok(RoutineActionStatus::Pending(_)) => {
                    self.sessions.cancel(action.clone()).await?;
                }
            }
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
