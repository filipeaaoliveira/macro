use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CreateTaskError, NewTask};
use crate::DocumentStorageServiceClient;

fn follow_up() -> NewTask {
    NewTask {
        task_name: "Follow-up: the cache never expires".to_string(),
        markdown: "Found in review.".to_string(),
        project_id: Some(uuid::Uuid::from_u128(0xF00)),
        share_with_team: false,
        document_id: Some(uuid::Uuid::from_u128(0x101)),
    }
}

/// The task is made as the user, with the internal key in the standard
/// headers, in the project and with the id named.
#[tokio::test]
async fn a_task_is_made_as_the_user_in_its_project() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .and(header("x-internal-auth-key", "internal-key"))
        .and(header(
            "x-internal-macro-user-id",
            "macro|julia@example.com",
        ))
        .and(body_json(json!({
            "taskName": "Follow-up: the cache never expires",
            "markdown": "Found in review.",
            "projectId": "00000000-0000-0000-0000-000000000f00",
            "shareWithTeam": false,
            "documentId": "00000000-0000-0000-0000-000000000101"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "documentId": "00000000-0000-7000-8000-000000000101",
            "documentMetadata": {},
            "token": "t",
            "initialSnapshot": ""
        })))
        .expect(1)
        .mount(&server)
        .await;

    let made = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .create_task_for_user(&follow_up(), "macro|julia@example.com")
        .await
        .expect("made");

    assert_eq!(made, "00000000-0000-7000-8000-000000000101");
}

/// A refusal is told apart from an outage, which is worth trying again:
/// so are an unaccepted key or user, a timeout and throttling.
#[tokio::test]
async fn only_a_request_s_own_client_error_is_a_refusal() {
    for (status, refused) in [
        (400, true),
        (409, true),
        (401, false),
        (403, false),
        (408, false),
        (429, false),
        (503, false),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        let made = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
            .create_task_for_user(&follow_up(), "macro|julia@example.com")
            .await;

        match made {
            Err(CreateTaskError::Refused(_)) => assert!(refused, "{status}"),
            Err(CreateTaskError::Unavailable(_)) => assert!(!refused, "{status}"),
            Ok(_) => panic!("{status} made a task"),
        }
    }
}
