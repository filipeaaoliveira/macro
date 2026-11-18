use document_storage_service_client::DocumentStorageServiceClient;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::DocumentHoldTasks;
use crate::domain::main_hold::HoldTask;
use crate::domain::ports::HoldTasks;
use crate::outbound::task_project::fixture::{new_task_property_values, task_properties};

const RUN_TASK: &str = "00000000-0000-7000-8000-000000000101";
const NEW_TASK: &str = "00000000-0000-7000-8000-000000000202";
const INITIATIVE: &str = "00000000-0000-7000-8000-000000000303";

fn new_task() -> Uuid {
    Uuid::parse_str(NEW_TASK).expect("uuid")
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user")
}

fn task() -> HoldTask {
    HoldTask {
        title: "main is red in macro/app".to_string(),
        markdown: "Checks fail.".to_string(),
    }
}

/// The Run task's properties, with its Project, as document storage
/// answers for them.
fn properties() -> serde_json::Value {
    task_properties(RUN_TASK, INITIATIVE)
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
        .and(header("x-internal-auth-key", "internal-key"))
        .and(header(
            "x-internal-macro-user-id",
            "macro|julia@example.com",
        ))
        .and(body_partial_json(json!({
            "taskName": "main is red in macro/app",
            "markdown": "Checks fail.",
            "shareWithTeam": true,
            "propertyValues": new_task_property_values("macro|julia@example.com", Some(INITIATIVE)),
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
            new_task(),
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
        .and(body_partial_json(json!({ "shareWithTeam": true })))
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
            new_task(),
        )
        .await;
    assert!(filed.is_ok(), "{filed:?}");
    let requests = server.received_requests().await.expect("recorded");
    let created: serde_json::Value = requests
        .iter()
        .find(|request| request.url.path() == "/documents/create_task")
        .expect("created")
        .body_json()
        .expect("json");
    assert_eq!(
        created["propertyValues"],
        new_task_property_values("macro|julia@example.com", None),
        "{created}"
    );
    // It is made under the id its claim chose, so a retry finds it.
    assert_eq!(created["documentId"], json!(NEW_TASK), "{created}");
}

/// A Run task whose properties do not read has the hold's task filed in
/// no initiative, assigned to the owner and not started, as document
/// storage would file one with no properties.
#[tokio::test]
async fn properties_that_do_not_read_file_the_task_in_no_initiative() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/properties/entities/DOCUMENT/{RUN_TASK}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "entityId": RUN_TASK })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/documents/create_task"))
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
            new_task(),
        )
        .await;

    assert_eq!(filed.expect("filed"), new_task());
    let requests = server.received_requests().await.expect("recorded");
    let created: serde_json::Value = requests
        .iter()
        .find(|request| request.url.path() == "/documents/create_task")
        .expect("created")
        .body_json()
        .expect("json");
    assert_eq!(
        created["propertyValues"],
        new_task_property_values("macro|julia@example.com", None),
        "{created}"
    );
}

/// A Run task whose properties document storage could not read for now
/// (it failed, or refused the key) files nothing yet: the filing fails, to
/// be tried again, rather than file the task outside its project.
#[tokio::test]
async fn a_failed_properties_read_is_tried_again() {
    for status in [500, 503, 401] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/properties/entities/DOCUMENT/{RUN_TASK}")))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/documents/create_task"))
            .respond_with(created())
            .expect(0)
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
                new_task(),
            )
            .await;

        assert!(filed.is_err(), "{status}: {filed:?}");
    }
}

/// The task document under the claimed id, as document storage reads it,
/// deleted or not.
fn basic(deleted: bool) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "documentId": NEW_TASK,
        "documentName": "main is red in macro/app",
        "owner": "macro|julia@example.com",
        "deletedAt": if deleted { json!("2026-11-17T10:00:00Z") } else { json!(null) },
    }))
}

/// A task a filing whose answer was lost made is found under its id, even
/// deleted since, and not filed again; a refusal after which it is there is
/// that filing's conflict, and it is found too.
#[tokio::test]
async fn a_task_filed_under_its_id_is_found_not_filed_again() {
    for (before, after, posts) in [
        (Some(false), None, 0),
        (Some(true), None, 0),
        (None, Some(false), 1),
    ] {
        let server = MockServer::start().await;
        let read =
            Mock::given(method("GET")).and(path(format!("/internal/documents/{NEW_TASK}/basic")));
        match before {
            Some(deleted) => read.respond_with(basic(deleted)).mount(&server).await,
            None => {
                read.respond_with(ResponseTemplate::new(404))
                    .up_to_n_times(1)
                    .with_priority(1)
                    .mount(&server)
                    .await;
                if let Some(deleted) = after {
                    Mock::given(method("GET"))
                        .and(path(format!("/internal/documents/{NEW_TASK}/basic")))
                        .respond_with(basic(deleted))
                        .with_priority(2)
                        .mount(&server)
                        .await;
                }
            }
        }
        Mock::given(method("POST"))
            .and(path("/documents/create_task"))
            .respond_with(ResponseTemplate::new(409))
            .mount(&server)
            .await;

        let tasks = DocumentHoldTasks::new(DocumentStorageServiceClient::new(
            "internal-key".into(),
            server.uri(),
        ));
        let filed = tasks.file(&julia(), None, &task(), new_task()).await;

        assert_eq!(filed.expect("found"), new_task(), "{before:?} {after:?}");
        let made = server
            .received_requests()
            .await
            .expect("recorded")
            .iter()
            .filter(|request| request.method.as_str() == "POST")
            .count();
        assert_eq!(made, posts, "{before:?} {after:?}");
    }
}

/// What the factory comments on a hold's task as the owner is signed as
/// the factory's, so the owner's notes leave it out.
#[tokio::test]
async fn a_comment_is_signed_as_the_factory_s() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/documents/{NEW_TASK}/comments/text")))
        .and(body_partial_json(json!({
            "quote": "Failing checks",
            "content": "The base passes again.\n\nPosted by the software factory.",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    DocumentHoldTasks::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        server.uri(),
    ))
    .comment(
        &julia(),
        new_task(),
        "Failing checks",
        "The base passes again.",
    )
    .await
    .expect("commented");
}
