use models_properties::EntityType;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CreateTaskError, NewTask, NewTaskProperty, NewTaskPropertyValue, TaskEntityReference};
use crate::DocumentStorageServiceClient;

fn follow_up() -> NewTask {
    NewTask {
        task_name: "Follow-up: the cache never expires".to_string(),
        markdown: "Found in review.".to_string(),
        project_id: Some(uuid::Uuid::from_u128(0xF00)),
        share_with_team: false,
        document_id: Some(uuid::Uuid::from_u128(0x101)),
        ..NewTask::default()
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

/// A task is made as the acting user with its properties, the same
/// standard headers, no id of its own, and its document's id read back.
#[tokio::test]
async fn a_task_is_made_with_its_properties() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .and(header("x-internal-auth-key", "internal-key"))
        .and(header("x-internal-macro-user-id", "macro|user@example.com"))
        .and(body_json(json!({
            "taskName": "main is red",
            "markdown": "Checks fail.",
            "projectId": null,
            "shareWithTeam": true,
            "propertyValues": [{
                "propertyId": "00000001-0000-0000-0000-000000000014",
                "value": {
                    "type": "entity_reference",
                    "reference": { "entity_id": "initiative-1", "entity_type": "INITIATIVE" }
                }
            }]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "documentId": "task-1",
            "documentMetadata": {},
            "token": "t",
            "initialSnapshot": "",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let task = NewTask {
        property_values: vec![NewTaskProperty {
            property_id: "00000001-0000-0000-0000-000000000014".to_string(),
            value: NewTaskPropertyValue::EntityReference {
                reference: TaskEntityReference {
                    entity_id: "initiative-1".to_string(),
                    entity_type: EntityType::Initiative,
                },
            },
        }],
        ..NewTask::new("main is red", "Checks fail.")
    };
    let created = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .create_task_for_user(&task, "macro|user@example.com")
        .await
        .expect("created");
    assert_eq!(created, "task-1");
}

/// A task with no properties and no id of its own sends neither.
#[tokio::test]
async fn a_task_with_no_properties_sends_none() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .and(body_json(json!({
            "taskName": "main is red",
            "markdown": "",
            "projectId": null,
            "shareWithTeam": true,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "documentId": "task-2",
            "documentMetadata": {},
            "token": "t",
            "initialSnapshot": "",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let created = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .create_task_for_user(&NewTask::new("main is red", ""), "macro|user@example.com")
        .await
        .expect("created");
    assert_eq!(created, "task-2");
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

/// A comment on a passage is posted as the acting user.
#[tokio::test]
async fn a_comment_is_posted_on_the_passage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/task-1/comments/text"))
        .and(header(
            "x-document-storage-service-user-id",
            "macro|user@example.com",
        ))
        .and(body_json(
            json!({ "content": "Lifted.", "quote": "Failing checks:" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .comment_on_text(
            "task-1",
            "macro|user@example.com",
            "Failing checks:",
            "Lifted.",
        )
        .await
        .expect("commented");
}
