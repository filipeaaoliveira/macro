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
        share_with_team: true,
    }
}

/// The task is made as the user, with the internal key, in the project
/// named.
#[tokio::test]
async fn a_task_is_made_as_the_user_in_its_project() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .and(header(
            "x-document-storage-service-auth-key",
            "internal-key",
        ))
        .and(header(
            "x-document-storage-service-user-id",
            "macro|julia@example.com",
        ))
        .and(body_json(json!({
            "taskName": "Follow-up: the cache never expires",
            "markdown": "Found in review.",
            "projectId": "00000000-0000-0000-0000-000000000f00",
            "shareWithTeam": true
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

/// A refusal is told apart from an outage, which is worth trying again.
#[tokio::test]
async fn a_refusal_is_not_an_outage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let client = DocumentStorageServiceClient::new("internal-key".into(), server.uri());

    let refused = client
        .create_task_for_user(&follow_up(), "macro|julia@example.com")
        .await;
    let down = client
        .create_task_for_user(&follow_up(), "macro|julia@example.com")
        .await;

    assert!(matches!(refused, Err(CreateTaskError::Refused(status)) if status == 403));
    assert!(matches!(down, Err(CreateTaskError::Unavailable(_))));
}
