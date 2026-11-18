use channel_sender::ChannelSender;
use chrono::{DateTime, TimeZone, Utc};
use document_storage_service_client::DocumentStorageServiceClient;
use macro_user_id::user_id::MacroUserIdStr;
use messages::domain::models::{
    Message, MessageKind, MessageListItem, MessageParent, MessageThread, MessageThreadPreview,
    ThreadState,
};
use messages::domain::ports::MessagePage;
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::DocumentTaskNotes;
use crate::domain::ports::TaskNotes;

const OWNER: &str = "macro|julia@example.com";
const OTHER: &str = "macro|teo@example.com";
const TASK: Uuid = Uuid::from_u128(0x7a5c);

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 18, 9, minute, 0).unwrap()
}

fn owner() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from(OWNER.to_string()).expect("valid")
}

/// A comment of `sender` on the task, written at `minute`.
fn comment(id: u128, root: Option<u128>, sender: &str, minute: u32, text: &str) -> Message {
    Message {
        kind: MessageKind::Message,
        id: Uuid::from_u128(id),
        parent: MessageParent::Document(TASK.to_string().try_into().expect("document")),
        thread_id: root.map(Uuid::from_u128),
        sender_id: ChannelSender::try_from(sender.to_string()).expect("sender"),
        guest_name: None,
        imported_author: None,
        bot_profile: None,
        mentions: Vec::new(),
        triggered_by: None,
        triggered_via: None,
        content: text.to_string(),
        created_at: at(minute),
        updated_at: at(minute),
        edited_at: None,
        deleted_at: None,
        attachments: Vec::new(),
        reactions: Vec::new(),
    }
}

fn state(root: u128) -> ThreadState {
    ThreadState {
        root_id: Uuid::from_u128(root),
        user_id: OWNER.to_string(),
        resolved: false,
        anchor: None,
        created_at: at(0),
        updated_at: at(0),
        deleted_at: None,
    }
}

/// The owner's live comments since the Gate opened are their notes,
/// oldest first: a reply the preview left out is read from its thread,
/// and someone else's comment, an older one and a deleted one are not.
#[tokio::test]
async fn the_owner_s_comments_since_the_gate_are_their_notes() {
    let server = MockServer::start().await;
    let mut deleted = comment(0x15, Some(0x10), OWNER, 25, "Never mind");
    deleted.deleted_at = Some(at(26));
    let page = MessagePage {
        items: vec![
            MessageListItem {
                message: comment(0x10, None, OWNER, 5, "Old note, before the gate"),
                state: state(0x10),
                thread: MessageThreadPreview {
                    reply_count: 3,
                    latest_reply_at: Some(at(30)),
                    preview: vec![comment(0x11, Some(0x10), OTHER, 21, "Teo's idea")],
                },
            },
            MessageListItem {
                message: comment(0x20, None, OWNER, 22, "The cause: bwrap cannot mount /proc"),
                state: state(0x20),
                thread: MessageThreadPreview {
                    reply_count: 0,
                    latest_reply_at: None,
                    preview: Vec::new(),
                },
            },
        ],
        next_cursor: None,
        previous_cursor: None,
    };
    let thread = MessageThread {
        state: state(0x10),
        root: comment(0x10, None, OWNER, 5, "Old note, before the gate"),
        replies: vec![
            comment(0x11, Some(0x10), OTHER, 21, "Teo's idea"),
            deleted,
            comment(0x16, Some(0x10), OWNER, 30, "Use the host's /proc instead"),
        ],
    };
    Mock::given(method("GET"))
        .and(path(format!("/messages/document/{TASK}")))
        .and(header("x-document-storage-service-user-id", OWNER))
        .respond_with(ResponseTemplate::new(200).set_body_json(&page))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/messages/document/{TASK}/threads/{}",
            Uuid::from_u128(0x10)
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(&thread))
        .expect(1)
        .mount(&server)
        .await;

    let notes = DocumentTaskNotes::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        server.uri(),
    ))
    .owner_notes(TASK, &owner(), at(20))
    .await
    .expect("read");

    let texts: Vec<&str> = notes.iter().map(|note| note.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "The cause: bwrap cannot mount /proc",
            "Use the host's /proc instead"
        ]
    );
    assert_eq!(notes[0].written_at, at(22));
}

/// A task the owner no longer reads has no notes; document storage that
/// cannot answer is an error, for the Step to be tried again.
#[tokio::test]
async fn a_task_not_there_has_no_notes_and_an_outage_is_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/messages/document/{TASK}")))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let notes = DocumentTaskNotes::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        server.uri(),
    ));
    assert_eq!(
        notes
            .owner_notes(TASK, &owner(), at(20))
            .await
            .expect("none"),
        []
    );

    let down = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&down)
        .await;
    let notes = DocumentTaskNotes::new(DocumentStorageServiceClient::new(
        "internal-key".into(),
        down.uri(),
    ));
    assert!(notes.owner_notes(TASK, &owner(), at(20)).await.is_err());
}
