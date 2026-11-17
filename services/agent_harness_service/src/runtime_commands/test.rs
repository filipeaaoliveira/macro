use super::*;
use agent_harness::domain::error::{HarnessError, Result};
use agent_harness::domain::model::{CommandOutcome, HarnessCommand};
use agent_harness::domain::ports::{CommandForwarder as _, CommandTarget};
use agent_harness::outbound::forward::RedisCommandForwarder;
use agent_session::domain::model::AgentSessionId;
use agent_session::domain::session_command::{SessionCommand, SessionCommandOutput};
use std::sync::atomic::{AtomicUsize, Ordering};

struct RecordingHarness {
    calls: AtomicUsize,
    fails: bool,
}

impl ForwardedCommands for RecordingHarness {
    async fn execute_forwarded(
        &self,
        session_id: AgentSessionId,
        _command: HarnessCommand,
    ) -> Result<CommandOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fails {
            return Err(HarnessError::Disconnected(session_id));
        }
        Ok(CommandOutcome::Completed)
    }

    /// Runs nothing: answers with the command line as its output, failing
    /// with exit code 2, or as a runtime that runs no command.
    async fn run_forwarded_command(
        &self,
        session_id: AgentSessionId,
        command: SessionCommand,
    ) -> Result<SessionCommandOutput> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fails {
            return Err(HarnessError::Session(
                agent_session::domain::error::AgentSessionError::CommandUnsupported(session_id),
            ));
        }
        Ok(SessionCommandOutput::new(
            2,
            command.command.as_bytes(),
            b"",
        ))
    }
}

fn redis_client() -> redis::Client {
    let url = macro_env_var::optional_read_env_var("REDIS_URI")
        .ok()
        .flatten()
        .unwrap_or_else(|| "redis://127.0.0.1:6379".to_owned());
    redis::Client::open(url).expect("valid Redis URL")
}

async fn ready(readiness: &mut tokio::sync::watch::Receiver<bool>) {
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        readiness.wait_for(|ready| *ready),
    )
    .await
    .expect("consumer becomes ready")
    .expect("readiness channel remains open");
}

async fn wait_for_calls(harness: &RecordingHarness, expected: usize) {
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while harness.calls.load(Ordering::SeqCst) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("command is processed");
}

fn harness(fails: bool) -> Arc<RecordingHarness> {
    Arc::new(RecordingHarness {
        calls: AtomicUsize::new(0),
        fails,
    })
}

fn harness_id() -> HarnessId {
    HarnessId::new_from_uuid(macro_uuid::generate_uuid_v7())
}

#[tokio::test]
async fn a_command_executes_on_exactly_one_responsible_replica() {
    let redis = redis_client();
    let origin = ReplicaId::mint();
    let peer = ReplicaId::mint();
    let origin_harness = harness(false);
    let peer_harness = harness(false);
    let (origin_ready, mut origin_readiness) = tokio::sync::watch::channel(false);
    let origin_consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        origin,
        Arc::new(|_| false),
        Arc::clone(&origin_harness),
        origin_ready,
        empty_models(redis.clone()),
    ));
    let (peer_ready, mut peer_readiness) = tokio::sync::watch::channel(false);
    let peer_consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        peer,
        Arc::new(|_| false),
        Arc::clone(&peer_harness),
        peer_ready,
        empty_models(redis.clone()),
    ));
    ready(&mut origin_readiness).await;
    ready(&mut peer_readiness).await;

    let outcome = RedisCommandForwarder::new(redis)
        .forward(
            AgentSessionId::TEST_A,
            HarnessCommand::Delete,
            CommandTarget::Replica(peer),
        )
        .await
        .unwrap();

    assert_eq!(outcome, CommandOutcome::Completed);
    wait_for_calls(&peer_harness, 1).await;
    assert_eq!(origin_harness.calls.load(Ordering::SeqCst), 0);
    origin_consumer.abort();
    peer_consumer.abort();
}

#[tokio::test]
async fn a_responsible_replicas_error_does_not_fail_the_publish() {
    let redis = redis_client();
    let replica = ReplicaId::mint();
    let harness = harness(true);
    let (ready_tx, mut readiness) = tokio::sync::watch::channel(false);
    let consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        replica,
        Arc::new(|_| false),
        Arc::clone(&harness),
        ready_tx,
        empty_models(redis.clone()),
    ));
    ready(&mut readiness).await;

    let outcome = RedisCommandForwarder::new(redis)
        .forward(
            AgentSessionId::TEST_A,
            HarnessCommand::Delete,
            CommandTarget::Replica(replica),
        )
        .await
        .expect("the command publishes");

    assert_eq!(outcome, CommandOutcome::Completed);
    wait_for_calls(&harness, 1).await;
    consumer.abort();
}

#[tokio::test]
async fn publishing_without_an_owner_still_succeeds() {
    let redis = redis_client();
    let harness = harness(false);
    let (ready_tx, mut readiness) = tokio::sync::watch::channel(false);
    let consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        ReplicaId::mint(),
        Arc::new(|_| false),
        Arc::clone(&harness),
        ready_tx,
        empty_models(redis.clone()),
    ));
    ready(&mut readiness).await;

    let target = harness_id();
    let outcome = RedisCommandForwarder::new(redis)
        .forward(
            AgentSessionId::TEST_A,
            HarnessCommand::Delete,
            CommandTarget::Harness(target),
        )
        .await
        .expect("the command publishes");

    assert_eq!(outcome, CommandOutcome::Completed);
    assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
    consumer.abort();
}

#[tokio::test]
async fn overlapping_harness_connections_execute_once() {
    let redis = redis_client();
    let first = harness(false);
    let second = harness(false);
    let target = harness_id();
    let (first_ready, mut first_readiness) = tokio::sync::watch::channel(false);
    let first_consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        ReplicaId::mint(),
        Arc::new(move |harness| harness == target),
        Arc::clone(&first),
        first_ready,
        empty_models(redis.clone()),
    ));
    let (second_ready, mut second_readiness) = tokio::sync::watch::channel(false);
    let second_consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        ReplicaId::mint(),
        Arc::new(move |harness| harness == target),
        Arc::clone(&second),
        second_ready,
        empty_models(redis.clone()),
    ));
    ready(&mut first_readiness).await;
    ready(&mut second_readiness).await;

    RedisCommandForwarder::new(redis)
        .forward(
            AgentSessionId::TEST_A,
            HarnessCommand::Delete,
            CommandTarget::Harness(target),
        )
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while first.calls.load(Ordering::SeqCst) + second.calls.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("command is processed");
    assert_eq!(
        first.calls.load(Ordering::SeqCst) + second.calls.load(Ordering::SeqCst),
        1
    );
    first_consumer.abort();
    second_consumer.abort();
}

fn empty_models(redis: redis::Client) -> MacrodModels {
    MacrodModels::new(
        agent_harness::outbound::runtime_registry::RuntimeRegistry::new(),
        redis,
        std::time::Duration::from_secs(2),
    )
}

fn check() -> SessionCommand {
    SessionCommand {
        cwd: ".".into(),
        command: "just check".into(),
        timeout_secs: 5,
    }
}

/// A command for a session a peer holds runs on that peer alone, and its
/// end comes back to the replica that forwarded it.
#[tokio::test]
async fn a_forwarded_command_runs_on_the_peer_and_its_end_comes_back() {
    let redis = redis_client();
    let origin = ReplicaId::mint();
    let peer = ReplicaId::mint();
    let origin_harness = harness(false);
    let peer_harness = harness(false);
    let (origin_ready, mut origin_readiness) = tokio::sync::watch::channel(false);
    let origin_consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        origin,
        Arc::new(|_| false),
        Arc::clone(&origin_harness),
        origin_ready,
        empty_models(redis.clone()),
    ));
    let (peer_ready, mut peer_readiness) = tokio::sync::watch::channel(false);
    let peer_consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        peer,
        Arc::new(|_| false),
        Arc::clone(&peer_harness),
        peer_ready,
        empty_models(redis.clone()),
    ));
    ready(&mut origin_readiness).await;
    ready(&mut peer_readiness).await;

    let output = RedisCommandForwarder::new(redis)
        .run_command(AgentSessionId::TEST_A, check(), peer)
        .await
        .unwrap();

    assert_eq!(output, SessionCommandOutput::new(2, b"just check", b""));
    assert_eq!(peer_harness.calls.load(Ordering::SeqCst), 1);
    assert_eq!(origin_harness.calls.load(Ordering::SeqCst), 0);
    origin_consumer.abort();
    peer_consumer.abort();
}

/// A peer whose session runs no command says so, typed, across the bus.
#[tokio::test]
async fn a_peer_that_runs_no_command_says_so() {
    let redis = redis_client();
    let peer = ReplicaId::mint();
    let peer_harness = harness(true);
    let (peer_ready, mut peer_readiness) = tokio::sync::watch::channel(false);
    let peer_consumer = tokio::spawn(consume_runtime_commands(
        redis.clone(),
        peer,
        Arc::new(|_| false),
        Arc::clone(&peer_harness),
        peer_ready,
        empty_models(redis.clone()),
    ));
    ready(&mut peer_readiness).await;

    let refused = RedisCommandForwarder::new(redis)
        .run_command(AgentSessionId::TEST_A, check(), peer)
        .await
        .unwrap_err();

    assert!(
        matches!(
            refused,
            HarnessError::Session(
                agent_session::domain::error::AgentSessionError::CommandUnsupported(_)
            )
        ),
        "{refused:?}"
    );
    peer_consumer.abort();
}
