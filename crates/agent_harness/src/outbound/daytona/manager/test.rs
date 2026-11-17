use std::collections::HashMap;

use wiremock::matchers::{body_partial_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::outbound::daytona::types::DaytonaApiKey;

fn session() -> AgentSessionId {
    AgentSessionId::new_from_uuid(macro_uuid::Uuid::from_u128(7))
}

fn manager(server: &MockServer) -> DaytonaContainerManager {
    DaytonaContainerManager::new(
        DaytonaSettings {
            api_url: server.uri(),
            api_key: DaytonaApiKey::new("daytona-key".into()),
            snapshot: Snapshot::new("snapshot".into()),
            anthropic_api_key: AnthropicApiKey::new("anthropic-key".into()),
        },
        PendingCommands::new(),
    )
}

/// The session's sandbox, found by its label, started, and asked to run
/// the command, whose exit code comes back whatever it is.
async fn sandbox(server: &MockServer, exit_code: i32, result: &str) {
    let labels = serde_json::to_string(&HashMap::from([(
        SESSION_LABEL.to_owned(),
        session().to_string(),
    )]))
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/sandbox"))
        .and(query_param("labels", labels.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [{ "id": "sandbox-1", "state": "stopped", "errorReason": null }],
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/sandbox/sandbox-1/start"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sandbox/sandbox-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "sandbox-1", "state": "started", "errorReason": null,
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sandbox/sandbox-1/toolbox-proxy-url"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "url": format!("{}/toolbox", server.uri()),
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/toolbox/sandbox-1/process/execute"))
        .and(body_partial_json(serde_json::json!({
            "command": provision::session_command(&check()),
            "timeout": 90,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "exitCode": exit_code, "result": result,
        })))
        .expect(1)
        .mount(server)
        .await;
}

fn check() -> SessionCommand {
    SessionCommand {
        cwd: ".".into(),
        command: "just check".into(),
        timeout_secs: 60,
    }
}

#[tokio::test]
async fn a_command_runs_in_the_sessions_sandbox_and_hands_back_its_exit_code() {
    let server = MockServer::start().await;
    sandbox(&server, 2, "error: 1 test failed").await;

    let output = manager(&server).exec(session(), check()).await.unwrap();

    assert_eq!(
        output,
        SessionCommandOutput {
            exit_code: 2,
            stdout: "error: 1 test failed".into(),
            stderr: String::new(),
        }
    );
}

#[tokio::test]
async fn a_sandbox_that_ran_a_command_is_reaped_only_once_it_idles_after_it() {
    let server = MockServer::start().await;
    sandbox(&server, 0, "ok").await;
    let manager = manager(&server);

    manager.exec(session(), check()).await.unwrap();

    let id = DaytonaSandboxId::new("sandbox-1".into());
    let now = Instant::now();
    assert!(
        manager
            .managed
            .containers
            .reap_stale(now, IDLE_TIMEOUT, |_| false)
            .is_empty()
    );
    assert_eq!(
        manager
            .managed
            .containers
            .reap_stale(now + IDLE_TIMEOUT, IDLE_TIMEOUT, |_| false),
        [id]
    );
}

#[tokio::test]
async fn a_session_without_a_sandbox_runs_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sandbox"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "items": [] })))
        .mount(&server)
        .await;

    let error = manager(&server).exec(session(), check()).await.unwrap_err();

    assert!(matches!(error, HarnessError::Container(_)), "{error:?}");
}
