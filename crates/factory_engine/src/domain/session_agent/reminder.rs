//! The reminder of an agent whose turn ended with nothing handed back: one
//! later prompt in the same session, then the agent's last message for the
//! Run's owner when it ends with nothing handed back too.

use agent_runtime_protocol::domain::action::AgentActionId;
use agent_session::domain::routines::{
    PromptRoutineSession, RoutineSessionAction, RoutineSessions,
};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use super::{SessionAgent, SessionAgentError, prompt};
use crate::domain::models::{AgentAnswer, AgentTask};

/// What an agent whose turn ended with nothing handed back is told, once.
const REMINDER: &str = include_str!("remind.md");

/// The reminder's place among its Step's later prompts: before the first
/// round of checks, the earliest of them.
const REMINDER_PLACE: u16 = 1;

/// What a Step shows of an agent that stopped without a result and said
/// nothing.
pub(super) const SAID_NOTHING: &str = "The agent said nothing before it stopped.";

/// What a Step shows of an agent that stopped without a result when its
/// last message could not be read.
const UNREAD_LAST_WORDS: &str = "The agent's last message could not be read.";

impl<S: RoutineSessions, R: RunRepository> SessionAgent<S, R> {
    /// What the agent handed back for the Step `task`'s prompt `action`,
    /// whose turn ended; when nothing, what it hands back once reminded.
    /// The prompt reads as superseded once an earlier attempt reminded the
    /// agent, the only later prompt before the Step's answer: this attempt
    /// then waits on that reminder.
    pub(super) async fn answer_or_remind(
        &self,
        task: &AgentTask,
        action: &RoutineSessionAction,
        deadline: tokio::time::Instant,
    ) -> Result<AgentAnswer, SessionAgentError> {
        match self.handed_back(task.key, action.action_id.as_uuid()).await {
            Err(SessionAgentError::NothingHandedBack | SessionAgentError::Superseded) => {
                self.remind(task, action, deadline).await
            }
            handed => handed,
        }
    }

    /// Remind the agent, whose turn on `action` ended with nothing handed
    /// back, to hand back the Step `task`'s result, in the same session,
    /// and return what it hands back. The reminder is the Step's prompt at
    /// [`REMINDER_PLACE`], under an id made from the first prompt's, so a
    /// retried attempt records the same one, which keeps what was handed
    /// back since, and sends it again, which the session refuses as one it
    /// holds; then it waits on that turn. A reminder that also ends with
    /// nothing handed back is the agent stopping without a result, with its
    /// last message.
    async fn remind(
        &self,
        task: &AgentTask,
        action: &RoutineSessionAction,
        deadline: tokio::time::Instant,
    ) -> Result<AgentAnswer, SessionAgentError> {
        let session_id = action.session_id.as_uuid();
        let reminder = RoutineSessionAction {
            action_id: AgentActionId::from_uuid(reminder_id(action.action_id.as_uuid())),
            ..action.clone()
        };
        self.still_holds(task.key, session_id).await?;
        let began = self
            .repo
            .begin_turn(
                task.key,
                session_id,
                reminder.action_id.as_uuid(),
                REMINDER_PLACE,
                &task.outcomes,
            )
            .await
            .map_err(Self::records)?;
        // Left alone: the Step let go of this session, or another attempt
        // took it on to a later prompt, which a retry goes on from.
        if !began {
            self.still_holds(task.key, session_id).await?;
            return Err(SessionAgentError::Superseded);
        }
        tracing::info!("a factory step's agent ended its turn without a result: it is reminded");
        let prompted = self
            .sessions
            .follow_up(PromptRoutineSession {
                action: reminder.clone(),
                prompt: prompt(REMINDER, &task.outcomes),
                context: None,
            })
            .await;
        self.prompt_once(&reminder, prompted).await?;
        self.finished(&reminder, deadline).await?;

        match self
            .handed_back(task.key, reminder.action_id.as_uuid())
            .await
        {
            Err(SessionAgentError::NothingHandedBack) => Err(
                SessionAgentError::StoppedWithoutResult(self.last_words(action, &reminder).await),
            ),
            handed => handed,
        }
    }

    /// The agent's last message, for the Run's owner: what it said in its
    /// turn on the `reminder`, else in its turn on the first prompt,
    /// `action`. One that cannot be read says so: the owner decides all
    /// the same.
    async fn last_words(
        &self,
        action: &RoutineSessionAction,
        reminder: &RoutineSessionAction,
    ) -> String {
        for turn in [reminder, action] {
            match self.sessions.reply(turn.clone()).await {
                Ok(Some(said)) => return said,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(?error, "a factory step's agent's last message was not read");
                    return UNREAD_LAST_WORDS.to_string();
                }
            }
        }
        SAID_NOTHING.to_string()
    }
}

/// The id of the reminder of the prompt `action_id`: the same UUIDv7 with
/// its last random bit flipped, so every attempt of the Step makes the same
/// one, and it is never the prompt's own.
fn reminder_id(action_id: Uuid) -> Uuid {
    let mut bytes = action_id.into_bytes();
    bytes[15] ^= 1;
    Uuid::from_bytes(bytes)
}
