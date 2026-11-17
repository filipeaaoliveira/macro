use models_properties::api::requests::SetPropertyValue;
use models_properties::{EntityReference, EntityType};
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CreateTaskError, NewTask, NewTaskProperty};
use crate::DocumentStorageServiceClient;

/// A task is created as the acting user, with its properties, and its
/// document's id read back.
#[tokio::test]
async fn a_task_is_created_as_the_acting_user() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .and(header(
            "x-document-storage-service-auth-key",
            "internal-key",
        ))
        .and(header(
            "x-document-storage-service-user-id",
            "macro|user@example.com",
        ))
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
        task_name: "main is red".to_string(),
        markdown: "Checks fail.".to_string(),
        project_id: None,
        share_with_team: true,
        property_values: vec![NewTaskProperty {
            property_id: "00000001-0000-0000-0000-000000000014".to_string(),
            value: SetPropertyValue::EntityReference {
                reference: EntityReference::new("initiative-1", EntityType::Initiative),
            },
        }],
    };
    let created = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .create_task_for_user(&task, "macro|user@example.com")
        .await
        .expect("created");
    assert_eq!(created, "task-1");
}

/// A refusal is its own error, never a task.
#[tokio::test]
async fn a_refused_task_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let task = NewTask {
        task_name: "main is red".to_string(),
        markdown: String::new(),
        project_id: None,
        share_with_team: true,
        property_values: Vec::new(),
    };
    let created = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .create_task_for_user(&task, "macro|user@example.com")
        .await;
    assert!(
        matches!(created, Err(CreateTaskError::Refused(status)) if status == 403),
        "{created:?}"
    );
}

/// A failing document storage is worth trying again.
#[tokio::test]
async fn a_failing_document_storage_is_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let task = NewTask {
        task_name: "main is red".to_string(),
        markdown: String::new(),
        project_id: None,
        share_with_team: true,
        property_values: Vec::new(),
    };
    let created = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .create_task_for_user(&task, "macro|user@example.com")
        .await;
    assert!(
        matches!(created, Err(CreateTaskError::Unavailable(_))),
        "{created:?}"
    );
}
