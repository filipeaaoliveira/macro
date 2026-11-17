//! Redis implementation of [`CommandForwarder`].

use agent_session::domain::error::AgentSessionError;
use agent_session::domain::model::{AgentSessionId, ReplicaId};
use agent_session::domain::session_command::{SessionCommand, SessionCommandOutput};
use harness_id::HarnessId;
use redis::AsyncCommands as _;
use std::collections::HashMap;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

use crate::domain::error::{HarnessError, Result};
use crate::domain::model::{CommandOutcome, HarnessCommand};
use crate::domain::ports::{CommandForwarder, CommandTarget};

/// Broadcasts commands to the responsible harness replica over Redis.
#[derive(Clone)]
pub struct RedisCommandForwarder {
    redis: redis::Client,
}

impl RedisCommandForwarder {
    /// Build a Redis command forwarder.
    pub fn new(redis: redis::Client) -> Self {
        Self { redis }
    }
}

impl CommandForwarder for RedisCommandForwarder {
    async fn forward(
        &self,
        session: AgentSessionId,
        command: HarnessCommand,
        target: CommandTarget,
    ) -> Result<CommandOutcome> {
        let request = RuntimeCommandRequest {
            request_id: macro_uuid::Uuid::new_v4(),
            target: RuntimeCommandTarget::from(target),
            session,
            command,
            trace_context: current_trace_context(),
        };
        let payload = serde_json::to_string(&request)
            .map_err(|error| forward_error("serialize command request", error))?;
        let mut publisher = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| forward_error("open command publisher", error))?;
        publisher
            .publish::<_, _, ()>(COMMAND_CHANNEL, payload)
            .await
            .map_err(|error| forward_error("publish command request", error))?;
        Ok(CommandOutcome::Completed)
    }

    /// Subscribes to the request's own reply channel before publishing it,
    /// so the answer cannot arrive unheard, then waits for one answer for
    /// the command's timeout and [`RUN_COMMAND_MARGIN`].
    async fn run_command(
        &self,
        session: AgentSessionId,
        command: SessionCommand,
        replica: ReplicaId,
    ) -> Result<SessionCommandOutput> {
        let request_id = macro_uuid::Uuid::new_v4();
        let reply_channel = format!("{RUN_COMMAND_REPLY_PREFIX}{request_id}");
        let wait = command.timeout() + RUN_COMMAND_MARGIN;
        let request = RuntimeRunCommandRequest {
            request_id,
            replica: replica.as_uuid(),
            session,
            run: command,
            reply_channel: reply_channel.clone(),
            trace_context: current_trace_context(),
        };
        let payload = serde_json::to_string(&request)
            .map_err(|error| forward_error("serialize command run request", error))?;

        let mut replies = self
            .redis
            .get_async_pubsub()
            .await
            .map_err(|error| forward_error("open command reply subscriber", error))?;
        replies
            .subscribe(&reply_channel)
            .await
            .map_err(|error| forward_error("subscribe to command reply", error))?;
        let mut publisher = self
            .redis
            .get_multiplexed_async_connection()
            .await
            .map_err(|error| forward_error("open command publisher", error))?;
        publisher
            .publish::<_, _, ()>(COMMAND_CHANNEL, payload)
            .await
            .map_err(|error| forward_error("publish command run request", error))?;

        let mut messages = replies.on_message();
        let message = tokio::time::timeout(wait, futures::StreamExt::next(&mut messages))
            .await
            .map_err(|_| forward_error("wait for command reply", "no replica answered in time"))?
            .ok_or_else(|| forward_error("wait for command reply", "the subscription ended"))?;
        let payload: String = message
            .get_payload()
            .map_err(|error| forward_error("read command reply", error))?;
        let reply: RunCommandReply = serde_json::from_str(&payload)
            .map_err(|error| forward_error("parse command reply", error))?;
        reply.into_result(session)
    }
}

/// How much longer than a command's own timeout its forwarding replica
/// waits for the answer: the provider's own margin, and the hops between.
pub const RUN_COMMAND_MARGIN: std::time::Duration = std::time::Duration::from_secs(120);

/// The prefix of the channel a command run's answer is published on, the
/// request's id after it.
pub const RUN_COMMAND_REPLY_PREFIX: &str = "agent-harness.runtime-commands.reply.";

/// A command to run in a session's checkout, broadcast for the replica
/// that holds the session, which publishes how it ended on
/// `reply_channel`.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RuntimeRunCommandRequest {
    request_id: macro_uuid::Uuid,
    replica: macro_uuid::Uuid,
    session: AgentSessionId,
    run: SessionCommand,
    reply_channel: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    trace_context: HashMap<String, String>,
}

impl RuntimeRunCommandRequest {
    /// The replica the command is for.
    #[must_use]
    pub fn replica(&self) -> macro_uuid::Uuid {
        self.replica
    }

    /// The session whose checkout it runs in.
    #[must_use]
    pub fn session(&self) -> AgentSessionId {
        self.session
    }

    /// The channel its answer goes to.
    #[must_use]
    pub fn reply_channel(&self) -> &str {
        &self.reply_channel
    }

    /// The command to run.
    #[must_use]
    pub fn command(&self) -> &SessionCommand {
        &self.run
    }

    /// Create a consumer span parented to the sender's propagated context.
    #[must_use]
    pub fn processing_span(&self) -> tracing::Span {
        consumer_span(self.request_id, self.session, &self.trace_context)
    }
}

/// How a forwarded command run ended, as the replica that ran it publishes
/// it: no error text crosses but what the harness's own error says.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunCommandReply {
    /// It ran, with this end.
    Ran(SessionCommandOutput),
    /// The session's runtime runs no command.
    Unsupported,
    /// It could not run.
    Failed(String),
}

impl RunCommandReply {
    /// The answer to publish for `result`.
    #[must_use]
    pub fn of(result: &Result<SessionCommandOutput>) -> Self {
        match result {
            Ok(output) => Self::Ran(output.clone()),
            Err(HarnessError::Session(AgentSessionError::CommandUnsupported(_))) => {
                Self::Unsupported
            }
            Err(error) => Self::Failed(error.to_string()),
        }
    }

    fn into_result(self, session: AgentSessionId) -> Result<SessionCommandOutput> {
        match self {
            Self::Ran(output) => Ok(output),
            Self::Unsupported => Err(HarnessError::Session(
                AgentSessionError::CommandUnsupported(session),
            )),
            Self::Failed(why) => Err(forward_error("run the command on the peer", why)),
        }
    }
}

/// Redis channel broadcasting runtime commands to every harness replica.
pub const COMMAND_CHANNEL: &str = "agent-harness.runtime-commands";

/// A command broadcast received from another replica.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct RuntimeCommandRequest {
    request_id: macro_uuid::Uuid,
    target: RuntimeCommandTarget,
    session: AgentSessionId,
    command: HarnessCommand,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    trace_context: HashMap<String, String>,
}

/// The process-local destination selected by the command sender.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCommandTarget {
    /// A specific session-owning replica.
    Replica(macro_uuid::Uuid),
    /// Whichever replica currently holds this harness runtime connection.
    Harness(HarnessId),
}

impl RuntimeCommandRequest {
    /// The unique ID used to claim and trace this command.
    #[must_use]
    pub fn request_id(&self) -> macro_uuid::Uuid {
        self.request_id
    }

    /// The replica or harness connection selected for execution.
    #[must_use]
    pub fn target(&self) -> RuntimeCommandTarget {
        self.target
    }

    /// The session receiving the command.
    #[must_use]
    pub fn session(&self) -> AgentSessionId {
        self.session
    }

    /// Consume the transport envelope and return its domain command.
    #[must_use]
    pub fn into_command(self) -> HarnessCommand {
        self.command
    }

    /// Create a consumer span parented to the sender's propagated context.
    #[must_use]
    pub fn processing_span(&self) -> tracing::Span {
        runtime_command_span(self)
    }
}

impl From<CommandTarget> for RuntimeCommandTarget {
    fn from(target: CommandTarget) -> Self {
        match target {
            CommandTarget::Replica(replica) => Self::Replica(replica.as_uuid()),
            CommandTarget::Harness(harness) => Self::Harness(harness),
        }
    }
}

fn current_trace_context() -> HashMap<String, String> {
    let context = tracing::Span::current().context();
    let mut carrier = HashMap::new();
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&context, &mut carrier);
    });
    carrier
}

fn runtime_command_span(request: &RuntimeCommandRequest) -> tracing::Span {
    consumer_span(request.request_id, request.session, &request.trace_context)
}

fn consumer_span(
    request_id: macro_uuid::Uuid,
    session: AgentSessionId,
    trace_context: &HashMap<String, String>,
) -> tracing::Span {
    let span = tracing::info_span!(
        "redis.process",
        otel.kind = "consumer",
        messaging.system = "redis",
        messaging.operation.name = "process",
        messaging.operation.type = "process",
        messaging.message.id = %request_id,
        agent.session.id = %session,
    );
    let parent = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(trace_context)
    });
    let _ = span.set_parent(parent);
    span
}

fn forward_error(operation: &'static str, error: impl std::fmt::Display) -> HarnessError {
    HarnessError::Forward(rootcause::report!("{operation}: {error}").into_dynamic())
}
