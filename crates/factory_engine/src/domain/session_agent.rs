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
    RoutineSessionAction, RoutineSessionError, RoutineSessions, ValidateRoutineSession,
};
use bot_id::BotId;
use factory_records::domain::models::{Outcome, StepSession};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use crate::domain::models::{AgentAnswer, AgentTask};
use crate::domain::ports::StepAgent;

/// What the agent is told about handing its result back.
const HAND_BACK: &str = include_str!("session_agent/hand_back.md");

/// How long to wait before the first look at the session, and the longest
/// wait between two looks.
const FIRST_LOOK: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(10);

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

    /// Wait for the first prompt's turn to end.
    async fn finished(&self, action: &RoutineSessionAction) -> Result<(), SessionAgentError> {
        let started = tokio::time::Instant::now();
        let mut wait = FIRST_LOOK;
        loop {
            match self.sessions.status(action.clone()).await? {
                RoutineActionStatus::Succeeded => return Ok(()),
                RoutineActionStatus::Failed(reason) => {
                    return Err(SessionAgentError::TurnFailed(reason));
                }
                RoutineActionStatus::Pending(_) => {}
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

    #[tracing::instrument(err, skip_all, fields(run_id = %task.run_id, step = %task.step))]
    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, SessionAgentError> {
        // A retried Step goes on in the session its first attempt claimed,
        // so the work is never started twice.
        let claimed = self
            .repo
            .claim_session(
                task.run_id,
                &task.entry,
                task.visit,
                StepSession {
                    session_id: Uuid::now_v7(),
                    action_id: Uuid::now_v7(),
                },
                &task.outcomes,
            )
            .await
            .map_err(Self::records)?;
        let action = RoutineSessionAction {
            owner: task.owner.clone(),
            bot_id: self.bot_id,
            session_id: AgentSessionId::new_from_uuid(claimed.session.session_id),
            action_id: AgentActionId::from_uuid(claimed.session.action_id),
        };

        // The first attempt opens the session; a retry opens it only when
        // the session cannot be read, as when the attempt failed opening it.
        let opened = !claimed.new && self.sessions.status(action.clone()).await.is_ok();
        if !opened {
            self.sessions
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
                .await?;
        }

        // A prompt the session already has is a conflict, not a failure:
        // an earlier attempt sent it.
        let prompted = self
            .sessions
            .prompt(PromptRoutineSession {
                action: action.clone(),
                prompt: prompt(&task),
                context: None,
            })
            .await;
        match prompted {
            Ok(_) | Err(RoutineSessionError::Conflict) => {}
            Err(error) => return Err(error.into()),
        }

        self.finished(&action).await?;
        let handed = self
            .repo
            .handed_back(task.run_id, &task.entry, task.visit)
            .await
            .map_err(Self::records)?
            .ok_or(SessionAgentError::NothingHandedBack)?;
        Ok(AgentAnswer {
            outcome: handed.outcome,
            text: handed.output,
        })
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
