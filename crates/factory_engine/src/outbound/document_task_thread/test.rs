use channel_sender::ChannelSender;
use chrono::{TimeZone, Utc};
use document_storage_service_client::DocumentStorageServiceClient;
use factory_records::domain::models::RunId;
use macro_user_id::user_id::MacroUserIdStr;
use messages::domain::models::{
    Message, MessageKind, MessageListItem, MessageParent, MessageThreadPreview, ThreadState,
};
use messages::domain::ports::MessagePage;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::DocumentTaskThread;
use crate::domain::ports::{TaskThread, ThreadPosted};
use crate::domain::task_thread::ThreadComment;

const OWNER: &str = "macro|julia@example.com";
const TASK: Uuid = Uuid::from_u128(0x7a5c);
const OTHER: &str = "macro|teo@example.com";

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

fn thread(server: &MockServer) -> DocumentTaskThread {
    DocumentTaskThread::new(
        DocumentStorageServiceClient::new("internal-key".into(), server.uri()),
        "https://macro.test/",
    )
}

/// An update is posted on the task as its owner, under its id, in its
/// thread, notifying no one; one posted before under that id is posted
/// all the same.
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
        .and(query_param("notification_policy", "Silent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(409))
        .mount(&server)
        .await;
    let thread = thread(&server);

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
    let thread = thread(&server);

    let posted = thread
        .post(&owner(), TASK, &reply())
        .await
        .expect("answered");
    assert_eq!(posted, ThreadPosted::NoThread);
}

/// A root comment on the task, of `sender`, saying `text`.
fn root(id: u128, sender: &str, text: &str) -> MessageListItem {
    let at = Utc.with_ymd_and_hms(2026, 11, 18, 9, 0, 0).unwrap();
    MessageListItem {
        message: Message {
            kind: MessageKind::Message,
            id: Uuid::from_u128(id),
            parent: MessageParent::Document(TASK.to_string().try_into().expect("document")),
            thread_id: None,
            sender_id: ChannelSender::try_from(sender.to_string()).expect("sender"),
            guest_name: None,
            imported_author: None,
            bot_profile: None,
            mentions: Vec::new(),
            triggered_by: None,
            triggered_via: None,
            content: text.to_string(),
            created_at: at,
            updated_at: at,
            edited_at: None,
            deleted_at: None,
            attachments: Vec::new(),
            reactions: Vec::new(),
        },
        state: ThreadState {
            root_id: Uuid::from_u128(id),
            user_id: sender.to_string(),
            resolved: false,
            anchor: None,
            created_at: at,
            updated_at: at,
            deleted_at: None,
        },
        thread: MessageThreadPreview {
            reply_count: 0,
            latest_reply_at: None,
            preview: Vec::new(),
        },
    }
}

/// A Run's thread is the discussion the owner started with the factory's
/// comment that links the Run: not one another person started, nor one
/// without the factory's signature, nor another Run's.
#[tokio::test]
async fn a_run_s_thread_is_found_by_the_link_its_first_comment_carries() {
    let server = MockServer::start().await;
    let run_id: RunId = "0193a1e8-a9d0-7c1d-b114-175185b044c8"
        .parse()
        .expect("run id");
    let link = "https://macro.test/app/factory/runs/0193a1e8-a9d0-7c1d-b114-175185b044c8";
    let signed = |text: &str| format!("{text}\n\nPosted by the software factory.");
    let page = MessagePage {
        items: vec![
            root(0x31, OTHER, &signed(&format!("[Open the run]({link})"))),
            root(0x32, OWNER, &format!("[Open the run]({link})")),
            root(
                0x33,
                OWNER,
                &signed("[Open the run](https://macro.test/app/factory/runs/other)"),
            ),
            root(
                0x34,
                OWNER,
                &signed(&format!(
                    "**A factory run started.** [Open the run]({link})"
                )),
            ),
        ],
        next_cursor: None,
        previous_cursor: None,
    };
    Mock::given(method("GET"))
        .and(path(format!("/messages/document/{TASK}")))
        .and(header("x-document-storage-service-user-id", OWNER))
        .respond_with(ResponseTemplate::new(200).set_body_json(&page))
        .mount(&server)
        .await;
    let thread = thread(&server);

    assert_eq!(thread.run_link(run_id), link);
    let found = thread
        .find_thread(&owner(), TASK, link)
        .await
        .expect("read");
    assert_eq!(found, Some(Uuid::from_u128(0x34)));
}
