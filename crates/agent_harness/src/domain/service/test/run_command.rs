//! A command run in a session's checkout, on the replica that holds it.

use agent_session::domain::ports::AgentSessionNotificationRecipient as _;
use agent_session::domain::session_command::{SessionCommand, SessionCommandOutput};

use super::*;

fn check() -> SessionCommand {
    SessionCommand {
        cwd: ".".to_owned(),
        command: "just check".to_owned(),
        timeout_secs: 60,
    }
}

/// A session nobody else holds runs the command here, in its container.
#[tokio::test]
async fn a_command_runs_here_for_a_session_no_peer_holds() {
    let (service, repo, containers, _announcer, _runtimes) = harness();
    let id = disconnected_session(&repo, &containers).await;

    let output = service.run_command(id, check()).await.expect("runs");

    assert_eq!(output, SessionCommandOutput::new(0, b"just check", b""));
    assert_eq!(containers.commands(), [(id, check())]);
}

/// A session a live peer holds runs the command there, and its end comes
/// back: nothing runs here.
#[tokio::test]
async fn a_command_for_a_peer_held_session_runs_on_the_peer() {
    let repo = InMemoryAgentSessionRepo::new();
    let session = agent_session::testing::test_agent_session(AgentSessionId::new());
    let id = session.id;
    repo.insert_session(session);
    let claim = claim_as_peer(&repo, id).await;
    let forwarder = RecordingForwarder::default();
    let containers = MockContainerManager::new();
    let service = AgentHarnessService::new(
        None,
        AgentSessionServiceImpl::new(
            repo.clone(),
            FoldedMessageService::new(repo.clone()),
            NoOpRealtime,
            NoOpAgentSessionNameGenerator,
            Arc::new(NoOpTurnObserver),
            Arc::new(NoopLifecyclePublisher),
            ReplicaId::mint(),
        ),
        containers.clone(),
        AnnouncerMock::new(),
        TestConnections::new(
            MirrorBindings::default(),
            RuntimeRegistry::<ContainerSender>::new(),
        ),
        PromptContextMock::default(),
        PromptComposerMock::default(),
        EgressProvisionerMock::new(),
        forwarder.clone(),
        KindDefaultPolicies,
        HarnessDefaultCodingAgents,
        SessionDefaults {
            bot_id: BotId::TEST_A,
            model: "claude".to_owned(),
            harness: "opencode".to_owned(),
            repo_url: SessionRepository::parse("https://github.com/macro-inc/macro"),
        },
        NoopLifecyclePublisher,
        crate::domain::pending::PendingCommands::new(),
        crate::domain::ports::NoPromptMentions,
        crate::domain::ports::NoopAgentSessionNotifier,
    );

    let output = service.run_command(id, check()).await.expect("runs");

    assert_eq!(output, SessionCommandOutput::new(3, b"on the peer", b""));
    assert_eq!(*forwarder.runs.lock().unwrap(), [(claim.replica, id)]);
    assert!(containers.commands().is_empty());
}
