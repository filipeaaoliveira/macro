use super::*;
use crate::outbound::daytona::AnthropicApiKey;
use crate::testing::helpers::egress::test_egress;

/// One container per session, so a resume finds exactly one and `docker ps`
/// says which session it belongs to.
#[test]
fn a_session_names_one_container() {
    let session = AgentSessionId::TEST_A;

    assert_eq!(container_name(session), format!("macro-agent-{session}"));
}

/// On a shared network the sidecar keeps its own port and the container name
/// is its DNS name.
#[test]
fn a_sidecar_is_dialed_by_container_name() {
    assert_eq!(
        sidecar_address(&ContainerRef {
            name: "macro-agent-abc".to_owned(),
        }),
        format!("macro-agent-abc:{}", provision::SIDECAR_PORT)
    );
}

/// Same environment Daytona injects, so the readiness recipe is exercised
/// against what a deployed sandbox sees: the Anthropic key that activates the
/// one model provider `container/opencode.json` enables, plus the egress
/// variables the clone and every outbound call go through. No GitHub
/// credential - the proxy holds that one.
#[test]
fn sandbox_env_carries_the_model_key_and_egress() {
    let env = sandbox_env(
        &AnthropicApiKey::new("test-anthropic-key".to_owned()),
        test_egress(),
    );

    assert!(env.contains(&(
        "ANTHROPIC_API_KEY".to_owned(),
        "test-anthropic-key".to_owned()
    )));
    assert!(
        !env.iter()
            .any(|(key, _)| key == "GITHUB_TOKEN" || key == "REPO_URL")
    );
    for (key, value) in test_egress().environment() {
        assert!(env.contains(&(key, value)));
    }
}

/// Against a real daemon: the command runs in the checkout, in the
/// directory it names, its streams come back apart with its exit code, and
/// its timeout stops it.
#[tokio::test]
#[ignore = "needs a Docker daemon with debian:bookworm-slim pulled"]
async fn a_command_runs_in_the_sessions_container_with_its_streams_apart() {
    let session = AgentSessionId::new_from_uuid(macro_uuid::Uuid::now_v7());
    let docker = Docker::new("docker");
    let container = docker
        .run(&RunSpec {
            image: "debian:bookworm-slim".to_owned(),
            name: container_name(session),
            labels: vec![(SESSION_LABEL.to_owned(), session.to_string())],
            env: Vec::new(),
            network: "bridge".to_owned(),
        })
        .await
        .unwrap();
    docker
        .exec(
            &container,
            "mkdir -p /workspace/sub && echo marker > /workspace/sub/file",
            Duration::from_secs(30),
        )
        .await
        .unwrap();
    let manager = LocalContainerManager::new(LocalSettings {
        docker_binary: "docker".to_owned(),
        image: "debian:bookworm-slim".to_owned(),
        network: "bridge".to_owned(),
        anthropic_api_key: AnthropicApiKey::new("unused".to_owned()),
    });

    let failed = manager
        .exec(
            session,
            SessionCommand {
                cwd: "sub".to_owned(),
                command: "cat file; echo why >&2; exit 3".to_owned(),
                timeout_secs: 30,
            },
        )
        .await;
    let stopped = manager
        .exec(
            session,
            SessionCommand {
                cwd: ".".to_owned(),
                command: "sleep 30".to_owned(),
                timeout_secs: 1,
            },
        )
        .await;
    docker.remove(&container).await.unwrap();

    assert_eq!(
        failed.unwrap(),
        SessionCommandOutput {
            exit_code: 3,
            stdout: "marker\n".to_owned(),
            stderr: "why\n".to_owned(),
        }
    );
    assert_eq!(stopped.unwrap().exit_code, 124);
}
