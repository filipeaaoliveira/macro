//! Which comments on a task are its owner's own.

use chrono::{DateTime, TimeDelta, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use messages::domain::models::MessageListItem;
use serde_json::{Value, json};
use uuid::Uuid;

use super::owner_comments;

const TASK: &str = "00000000-0000-7000-8000-000000000101";

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

/// A comment as the message service serves it.
fn message(sender: &str, content: &str, at: DateTime<Utc>) -> Value {
    json!({
        "id": Uuid::now_v7(),
        "parent": { "type": "document", "id": TASK },
        "thread_id": null,
        "sender_id": sender,
        "imported_author": null,
        "bot_profile": null,
        "mentions": [],
        "triggered_by": null,
        "content": content,
        "created_at": at,
        "updated_at": at,
        "edited_at": null,
        "deleted_at": null,
        "attachments": [],
        "reactions": [],
    })
}

/// A discussion whose first comment is `root`, with the replies `replies`.
fn discussion(root: Value, replies: Vec<Value>) -> MessageListItem {
    let at = root["created_at"].clone();
    let mut item = root.clone();
    let fields = item.as_object_mut().expect("object");
    fields.insert(
        "state".to_string(),
        json!({
            "root_id": root["id"],
            "user_id": root["sender_id"],
            "resolved": false,
            "anchor": null,
            "created_at": at,
            "updated_at": at,
            "deleted_at": null,
        }),
    );
    fields.insert(
        "thread".to_string(),
        json!({
            "reply_count": replies.len(),
            "latest_reply_at": null,
            "preview": replies,
        }),
    );
    serde_json::from_value(item).expect("a discussion reads")
}

#[test]
fn only_what_the_owner_wrote_themselves_after_the_gate_opened_counts() {
    let opened = Utc::now();
    let later = |seconds| opened + TimeDelta::seconds(seconds);
    let mut by_an_agent = message(
        "macro|julia@example.com",
        "Approve, says the agent",
        later(4),
    );
    by_an_agent["triggered_via"] = json!("an agent client");
    let mut deleted = message("macro|julia@example.com", "Scrap that", later(5));
    deleted["deleted_at"] = json!(later(6));
    let items = [
        discussion(
            message("macro|teo@example.com", "Approve it", later(1)),
            vec![message("macro|julia@example.com", "Go ahead", later(3))],
        ),
        discussion(message("macro|julia@example.com", "Before", opened), vec![]),
        discussion(message("macro|julia@example.com", "Hmm", later(2)), vec![]),
        discussion(by_an_agent, vec![deleted]),
    ];

    let comments = owner_comments(&items, &julia(), opened);

    let texts: Vec<&str> = comments
        .iter()
        .map(|comment| comment.text.as_str())
        .collect();
    assert_eq!(texts, ["Hmm", "Go ahead"]);
}
