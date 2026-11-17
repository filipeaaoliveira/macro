use base64::Engine as _;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::outbound::namespace::types::{ImageRef, InstanceId, NamespaceToken};

fn session() -> AgentSessionId {
    AgentSessionId::new_from_uuid(macro_uuid::Uuid::from_u128(9))
}

fn manager(server: &MockServer) -> NamespaceContainerManager {
    NamespaceContainerManager::new(NamespaceSettings {
        api_url: server.uri(),
        token: NamespaceToken::new("namespace-token".into()),
        image_ref: ImageRef::new("image".into()),
        lifetime: std::time::Duration::from_secs(3600),
    })
}

fn check() -> SessionCommand {
    SessionCommand {
        cwd: ".".into(),
        command: "just check".into(),
        timeout_secs: 60,
    }
}

fn encoded(text: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(text)
}

/// Namespace answers the command's run with `exit_code`, or with none.
async fn answering(server: &MockServer, exit_code: Option<i32>) -> NamespaceContainerManager {
    Mock::given(method("POST"))
        .and(path(
            "/namespace.cloud.compute.v1beta.CommandService/RunCommandSync",
        ))
        .and(body_partial_json(serde_json::json!({
            "instanceId": "instance-1",
            "targetContainerName": "harness",
            "command": { "command": ["bash", "-c", check().script(provision::CHECKOUT)] },
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "stdout": encoded("1 failed"),
            "stderr": encoded("error: test failed"),
            "exitCode": exit_code,
        })))
        .expect(1)
        .mount(server)
        .await;
    let manager = manager(server);
    manager.instances.insert(
        session(),
        Arc::new(Instance {
            id: InstanceId::new("instance-1".into()),
            url: String::new(),
            command_endpoint: server.uri(),
        }),
    );
    manager
}

#[tokio::test]
async fn a_command_runs_in_the_instance_spawned_for_the_session() {
    let server = MockServer::start().await;
    let manager = answering(&server, Some(101)).await;

    let output = manager.exec(session(), check()).await.unwrap();

    assert_eq!(
        output,
        SessionCommandOutput {
            exit_code: 101,
            stdout: "1 failed".into(),
            stderr: "error: test failed".into(),
        }
    );
}

/// A run Namespace gave no exit code for is not a pass.
#[tokio::test]
async fn a_command_without_an_exit_code_fails() {
    let server = MockServer::start().await;
    let manager = answering(&server, None).await;

    let error = manager.exec(session(), check()).await.unwrap_err();

    assert!(matches!(error, HarnessError::Container(_)), "{error:?}");
}

#[tokio::test]
async fn a_session_this_process_did_not_spawn_runs_nothing() {
    let server = MockServer::start().await;

    let error = manager(&server).exec(session(), check()).await.unwrap_err();

    assert!(matches!(error, HarnessError::Container(_)), "{error:?}");
}

/// Tearing a session down destroys its instance and forgets it: no
/// command runs there afterwards.
#[tokio::test]
async fn teardown_forgets_the_session_s_instance() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(
            "/namespace.cloud.compute.v1beta.ComputeService/DestroyInstance",
        ))
        .and(body_partial_json(
            serde_json::json!({ "instanceId": "instance-1" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let manager = manager(&server);
    manager.instances.insert(
        session(),
        Arc::new(Instance {
            id: InstanceId::new("instance-1".into()),
            url: String::new(),
            command_endpoint: server.uri(),
        }),
    );

    manager.teardown(session()).await.unwrap();

    assert!(manager.instances.is_empty());
}
