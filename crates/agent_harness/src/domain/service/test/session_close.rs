//! Deleting an external session ends it on its runtime, so the agent
//! process its adapter keeps for the session does not outlive it.

use agent_client_protocol::schema::v1::{CloseSessionResponse, SessionCloseCapabilities};

use super::*;

/// [`complete_bound_handshake`] for an agent that advertises
/// `sessionCapabilities.close`.
async fn complete_closable_handshake(runtime: &ContainerMock) {
    let agent = runtime.agent();
    while agent.received_requests().is_empty() {
        runtime.sends_ready();
        tokio::task::yield_now().await;
    }
    agent.completes_initialize(
        InitializeResponse::new(PROTOCOL_VERSION).agent_capabilities(
            AgentCapabilities::new().session_capabilities(
                SessionCapabilities::new().close(SessionCloseCapabilities::new()),
            ),
        ),
    );
    agent.wait_for_requests(2).await;
    agent.opens_session(NewSessionResponse::new("acp-test"));
    agent.completes_prompt().await;
}

fn prompt() -> ControlEvent {
    ControlEvent {
        action: AgentAction::prompt("fix the failing test"),
        action_id: None,
        actor: Some(sender()),
        context: None,
        client: None,
        prompter: agent_session::domain::model::PrompterKind::User,
    }
}

#[tokio::test]
async fn deleting_an_external_session_closes_it_on_its_runtime() {
    let (service, repo, _containers, _announcer, runtimes) = harness();
    let session = service
        .open_external_session(open_external_request("/home/operator/code"))
        .await
        .unwrap();
    let runtime = ContainerMock::default();
    runtimes.attach(harness_for_bot(session.bot_id), runtime.clone());
    let (prompted, ()) = tokio::join!(
        service.control_event(session.id, prompt()),
        complete_closable_handshake(&runtime)
    );
    prompted.expect("the prompt reaches the external runtime");
    let agent = runtime.agent();

    let deleted = service.execute(session.id, HarnessCommand::Delete);
    let answered = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            agent.wait_for_method_requests("session/close", 1),
        )
        .await
        .expect("the delete sends session/close to the runtime");
        let close = agent
            .received_frames()
            .into_iter()
            .find_map(|frame| match frame {
                RawJsonRpcMessage::Request(request)
                    if request.method.as_ref() == "session/close" =>
                {
                    Some(request)
                }
                _ => None,
            })
            .expect("a session/close was sent");
        agent.sends_reply(close.id, CloseSessionResponse::new());
    };
    let (deleted, ()) = tokio::join!(deleted, answered);
    deleted.expect("the delete succeeds");

    let closes: Vec<_> = agent
        .received_requests()
        .into_iter()
        .filter_map(|request| match request {
            ClientRequest::CloseSessionRequest(close) => Some(close.session_id),
            _ => None,
        })
        .collect();
    assert_eq!(closes, [SessionId::new("acp-test")]);
    assert!(repo.get(session.id).await.is_err(), "the row is gone");
}

#[tokio::test]
async fn deleting_an_external_session_with_no_runtime_connected_still_deletes_it() {
    let (service, repo, _containers, _announcer, _runtimes) = harness();
    let session = service
        .open_external_session(open_external_request("/home/operator/code"))
        .await
        .unwrap();
    repo.set_acp_session_id(session.id, SessionId::new("acp-test"))
        .await
        .unwrap();

    service
        .inner
        .delete(session.id)
        .await
        .expect("a runtime that is gone does not fail the delete");

    assert!(repo.get(session.id).await.is_err(), "the row is gone");
}
