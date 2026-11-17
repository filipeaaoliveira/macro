use document_storage_service_client::DocumentStorageServiceClient;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::DocumentHoldTasks;
use crate::domain::main_hold::HoldTask;
use crate::domain::ports::HoldTasks;

const RUN_TASK: &str = "00000000-0000-7000-8000-000000000101";
const NEW_TASK: &str = "00000000-0000-7000-8000-000000000202";
const INITIATIVE: &str = "00000000-0000-7000-8000-000000000303";
const PROJECT: &str = "00000001-0000-0000-0000-000000000014";

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user")
}

fn task() -> HoldTask {
    HoldTask {
        title: "main is red in macro/app".to_string(),
        markdown: "Checks fail.".to_string(),
    }
}

/// The Run task's properties, with its Project.
fn properties() -> serde_json::Value {
    json!({
        "entityId": RUN_TASK,
        "properties": [{
            "property": {
                "id": "00000000-0000-7000-8000-000000000404",
                "entityId": RUN_TASK,
                "entityType": "TASK",
                "propertyDefinitionId": PROJECT,
            },
            "definition": {
                "id": PROJECT,
                "scope": "system",
                "displayName": "Project",
                "dataType": "ENTITY",
                "isMultiSelect": false,
                "specificEntityType": "INITIATIVE",
                "isSystem": true,
                "isMetadata": false,
            },
            "value": {
                "type": "EntityReference",
                "value": [{ "entity_id": INITIATIVE, "entity_type": "INITIATIVE" }],
            },
        }],
    })
}

fn created() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "documentId": NEW_TASK,
        "documentMetadata": {},
        "token": "t",
        "initialSnapshot": "",
    }))
}

/// The task is filed as the Run's owner in the initiative of the Run's own
/// task.
#[tokio::test]
async fn the_task_is_filed_in_the_initiative_of_the_run_s_task() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/properties/entities/DOCUMENT/{RUN_TASK}")))
        .and(header(
            "x-document-storage-service-user-id",
            "macro|julia@example.com",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(properties()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .and(header(
            "x-document-storage-service-user-id",
            "macro|julia@example.com",
        ))
        .and(body_partial_json(json!({
            "taskName": "main is red in macro/app",
            "markdown": "Checks fail.",
            "shareWithTeam": true,
            "propertyValues": [{
                "propertyId": PROJECT,
                "value": {
                    "type": "entity_reference",
                    "reference": { "entity_id": INITIATIVE, "entity_type": "INITIATIVE" },
                },
            }],
        })))
        .respond_with(created())
        .expect(1)
        .mount(&server)
        .await;

    let tasks = DocumentHoldTasks::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        server.uri(),
    ));
    let filed = tasks
        .file(
            &julia(),
            Some(Uuid::parse_str(RUN_TASK).expect("uuid")),
            &task(),
        )
        .await
        .expect("filed");
    assert_eq!(filed, Uuid::parse_str(NEW_TASK).expect("uuid"));
}

/// A Run task the owner no longer sees files the task in no initiative.
#[tokio::test]
async fn a_hidden_run_task_files_the_task_in_no_initiative() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/properties/entities/DOCUMENT/{RUN_TASK}")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
        .and(body_partial_json(json!({ "propertyValues": [] })))
        .respond_with(created())
        .expect(1)
        .mount(&server)
        .await;

    let tasks = DocumentHoldTasks::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        server.uri(),
    ));
    let filed = tasks
        .file(
            &julia(),
            Some(Uuid::parse_str(RUN_TASK).expect("uuid")),
            &task(),
        )
        .await;
    assert!(filed.is_ok(), "{filed:?}");
}
