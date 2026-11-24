use std::path::PathBuf;

use agent_session::domain::model::AgentSessionId;
use bot_id::BotId;
use harness_id::HarnessId;
use macro_user_id::user_id::MacroUserIdStr;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;

use super::{DispatchError, Dispatcher};
use crate::config::{HarnessCredentials, MacroApi, Workspace};
use crate::outbound::agent_session::HarnessApi;
use crate::runtime::Runtime;
use crate::trigger::{TriggerWork, WorkExecutor as _};
use crate::turns::ActiveTurns;

/// One request the fake service received: its request line and its body.
struct Received {
    line: String,
    body: serde_json::Value,
}

/// Answer one request on `listener` with 204, and hand it back.
async fn answer_one(listener: TcpListener) -> Received {
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = BufReader::new(stream);
    let mut line = String::new();
    stream.read_line(&mut line).await.unwrap();
    let mut length = 0;
    let mut header = String::new();
    loop {
        header.clear();
        stream.read_line(&mut header).await.unwrap();
        if header == "\r\n" {
            break;
        }
        if let Some(value) = header.to_ascii_lowercase().strip_prefix("content-length:") {
            length = value.trim().parse::<usize>().unwrap();
        }
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await.unwrap();
    stream
        .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    Received {
        line: line.trim_end().to_owned(),
        body: serde_json::from_slice(&body).unwrap(),
    }
}

/// A session requested for a repository this machine does not work in is
/// refused, and the refusal goes to the service with its reason, so the
/// requester stops waiting on a session that never opens.
#[tokio::test]
async fn a_session_for_another_repository_is_refused_to_the_service() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = HarnessApi::new(
        &MacroApi {
            egress_url: None,
            api_url: format!("http://{}", listener.local_addr().unwrap()),
            storage_url: "http://unused".to_owned(),
            web_url: "http://unused".to_owned(),
        },
        &HarnessCredentials {
            harness_id: HarnessId::TEST_A,
            token: "mhns_test".to_owned(),
            pairing_id: None,
        },
    );
    let workspace = tempfile::tempdir().unwrap();
    let dispatcher = Dispatcher::new(
        api,
        Runtime::never_connected(),
        Workspace {
            path: PathBuf::from(workspace.path()),
            repo_url: Some("https://github.com/acme/own.git".to_owned()),
        },
        ActiveTurns::default(),
    );
    let service = tokio::spawn(answer_one(listener));
    let session = AgentSessionId::new();

    let executed = dispatcher
        .execute(TriggerWork::OpenRequested {
            repo_url: Some("https://github.com/acme/other.git".to_owned()),
            work_branch: Some("factory/run".to_owned()),
            session,
            bot: BotId::TEST_A,
            sender: MacroUserIdStr::try_from_email("asker@example.com").unwrap(),
            origin: trigger_context::TriggerOrigin::Unknown,
        })
        .await;

    let Err(DispatchError::Workspace(reason)) = executed else {
        panic!("expected a workspace refusal, got {executed:?}");
    };
    let received = tokio::time::timeout(std::time::Duration::from_secs(5), service)
        .await
        .expect("the refusal reached the service")
        .unwrap();
    assert_eq!(
        received.line,
        format!("POST /agent-sessions/{session}/refusal HTTP/1.1")
    );
    assert_eq!(
        received.body,
        serde_json::json!({ "botId": BotId::TEST_A.as_uuid(), "reason": reason })
    );
    assert!(reason.contains("does not manage repositories"), "{reason}");
}
