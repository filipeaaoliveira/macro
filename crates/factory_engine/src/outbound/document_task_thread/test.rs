use document_storage_service_client::DocumentStorageServiceClient;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::DocumentTaskThread;
use crate::domain::ports::{TaskThread, ThreadPosted};
use crate::domain::task_thread::ThreadComment;

const OWNER: &str = "macro|julia@example.com";
const TASK: Uuid = Uuid::from_u128(0x7a5c);

fn owner() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from(OWNER.to_string()).expect("valid")
}

fn reply() -> ThreadComment {
    ThreadComment {
        id: Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0003),
        thread_id: Some(Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001)),
        text: "Step 2, `write`, ended `done`.\n\nPosted by the software factory.".to_string(),
    }
}

/// An update is posted on the task as its owner, under its id, in its
/// thread; one posted before under that id is posted all the same.
#[tokio::test]
async fn an_update_is_posted_as_the_owner_under_its_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(format!("/messages/document/{TASK}")))
        .and(header("x-document-storage-service-user-id", OWNER))
        .and(body_json(json!({
            "id": reply().id,
            "content": reply().text,
            "thread_id": reply().thread_id,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&server)
        .await;
    let thread = DocumentTaskThread::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        server.uri(),
    ));

    let posted = thread.post(&owner(), TASK, &reply()).await.expect("posted");
    assert_eq!(posted, ThreadPosted::Posted);
    let again = thread
        .post(&owner(), TASK, &reply())
        .await
        .expect("posted before");
    assert_eq!(again, ThreadPosted::Posted);
}

/// A thread, or a task, the owner does not reach is not there.
#[tokio::test]
async fn a_missing_thread_is_not_there() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let thread = DocumentTaskThread::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        server.uri(),
    ));

    let posted = thread
        .post(&owner(), TASK, &reply())
        .await
        .expect("answered");
    assert_eq!(posted, ThreadPosted::NoThread);
}
