//! A command run in a session's checkout, beside its agent, on the replica
//! that holds the session.
//!
//! Not a queued command: a check can run for many minutes, and the session's
//! other commands (a stop above all) must not wait behind it. It runs where
//! the session's live actor is, because that replica's provider keeps the
//! sandbox from being reaped while the command runs, and is the one that
//! knows a sandbox it spawned without a durable lookup.

use agent_session::domain::session_command::{SessionCommand, SessionCommandOutput};

use super::*;

impl<
    Sessions,
    Containers,
    Announcer,
    Runtimes,
    PromptContext,
    PromptComposer,
    Egress,
    Lifecycle,
    Mentions,
    Notifier,
>
    AgentHarnessInner<
        Sessions,
        Containers,
        Announcer,
        Runtimes,
        PromptContext,
        PromptComposer,
        Egress,
        Lifecycle,
        Mentions,
        Notifier,
    >
where
    Sessions: AgentSessionService,
    Containers: ContainerManager,
    Announcer: SessionAnnouncer,
    Runtimes: RuntimeConnections,
    PromptContext: MessagePromptContext,
    PromptComposer: AgentPromptComposer,
    Egress: SandboxEgressProvisioner,
    Lifecycle: AgentSessionLifecyclePublisher,
    Mentions: PromptMentions,
    Notifier: AgentSessionNotifier,
{
    /// Run `command` here when this replica holds the session or nobody
    /// does, and on the peer that holds it otherwise, waiting for its end.
    /// A draining replica runs nothing: the caller's retry lands on one that
    /// is staying.
    #[tracing::instrument(err, skip(self, command), fields(%session_id))]
    pub(super) async fn run_command(
        &self,
        session_id: AgentSessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandOutput> {
        match self.sessions.management(session_id).await? {
            SessionManagement::Draining => Err(AgentSessionError::Draining(session_id).into()),
            SessionManagement::Peer(manager) => {
                tracing::info!(%session_id, peer = %manager.replica, "forwarding a session command");
                self.forwarder
                    .run_command(session_id, command, manager.replica)
                    .await
            }
            SessionManagement::Ours | SessionManagement::Unmanaged => {
                self.containers.exec(session_id, command).await
            }
        }
    }
}
