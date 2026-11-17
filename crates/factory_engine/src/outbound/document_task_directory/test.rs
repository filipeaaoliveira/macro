//! Making a follow-up task against a stand-in for document storage.

use document_storage_service_client::DocumentStorageServiceClient;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::DocumentTaskDirectory;
use crate::domain::ports::{FollowUpMade, FollowUpTask};

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

fn follow_up() -> FollowUpTask {
    FollowUpTask {
        name: "Follow-up: docs name the old flag".to_string(),
        markdown: "Found in review.".to_string(),
    }
}

/// The task document `id`, as document storage reads it.
fn basic(id: Uuid) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "documentId": id.to_string(),
        "documentName": "Follow-up",
        "owner": "macro|julia@example.com",
        "deletedAt": null
    }))
}

/// Made as document storage answers: `id`'s read answering `found`, in
/// order, and the make answering `made`, if it is asked.
async fn make(id: Uuid, found: &[bool], made: Option<ResponseTemplate>) -> (FollowUpMade, usize) {
    let server = MockServer::start().await;
    for (index, there) in found.iter().enumerate() {
        let answer = if *there {
            basic(id)
        } else {
            ResponseTemplate::new(404)
        };
        let reads = Mock::given(method("GET"))
            .and(path(format!("/internal/documents/{id}/basic")))
            .respond_with(answer)
            .up_to_n_times(1);
        reads
            .with_priority(u8::try_from(index + 1).expect("few"))
            .mount(&server)
            .await;
    }
    if let Some(made) = made {
        Mock::given(method("POST"))
            .and(path("/documents/create_task"))
            .respond_with(made)
            .mount(&server)
            .await;
    }
    let directory = DocumentTaskDirectory {
        access: (),
        documents: DocumentStorageServiceClient::new("key".into(), server.uri()),
    };

    let outcome = directory
        .make_follow_up(&julia(), None, &follow_up(), id, async |_| Ok(true))
        .await
        .expect("answered");
    let posts = server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|request| request.method.as_str() == "POST")
        .count();
    (outcome, posts)
}

/// A task an earlier attempt made is found by its id, and not made again.
#[tokio::test]
async fn a_task_already_made_is_found_not_made_again() {
    let id = Uuid::now_v7();

    let (outcome, posts) = make(id, &[true], None).await;

    assert_eq!(outcome, FollowUpMade::Made);
    assert_eq!(posts, 0);
}

/// A refusal after which the task is there is the conflict of an attempt
/// that made it meanwhile; one after which it is not is a refusal.
#[tokio::test]
async fn a_conflict_is_made_when_the_task_is_there_and_refused_when_not() {
    let id = Uuid::now_v7();

    let (found, _) = make(id, &[false, true], Some(ResponseTemplate::new(409))).await;
    let (refused, _) = make(id, &[false, false], Some(ResponseTemplate::new(409))).await;

    assert_eq!(found, FollowUpMade::Made);
    assert_eq!(refused, FollowUpMade::Refused);
}

/// A task made under another id than the one asked for is told apart, so
/// the round stops rather than make one on every retry.
#[tokio::test]
async fn a_task_made_under_another_id_is_told_apart() {
    let id = Uuid::now_v7();
    let other = Uuid::now_v7();
    let answer =
        ResponseTemplate::new(200).set_body_json(json!({ "documentId": other.to_string() }));

    let (same, _) = make(
        id,
        &[false],
        Some(ResponseTemplate::new(200).set_body_json(json!({ "documentId": id.to_string() }))),
    )
    .await;
    let (elsewhere, _) = make(id, &[false], Some(answer)).await;

    assert_eq!(same, FollowUpMade::Made);
    assert_eq!(elsewhere, FollowUpMade::OtherId(other.to_string()));
}

/// A follow-up of a task in a project the owner may no longer add to is
/// refused without asking document storage, which would answer 401, as
/// for a key it does not accept; one they may add to goes in the project.
#[tokio::test]
async fn a_project_the_owner_may_not_add_to_refuses_the_follow_up() {
    let id = Uuid::now_v7();
    let of = Uuid::now_v7();
    let project = Uuid::now_v7();

    for may in [false, true] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/internal/documents/{id}/basic")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/internal/documents/{of}/basic")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "documentId": of.to_string(),
                "documentName": "Add dark mode",
                "owner": "macro|julia@example.com",
                "projectId": project.to_string(),
                "deletedAt": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/documents/create_task"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({ "documentId": id.to_string() })),
            )
            .mount(&server)
            .await;
        let directory = DocumentTaskDirectory {
            access: (),
            documents: DocumentStorageServiceClient::new("key".into(), server.uri()),
        };
        let asked = std::sync::Mutex::new(Vec::new());

        let outcome = directory
            .make_follow_up(&julia(), Some(of), &follow_up(), id, async |checked| {
                asked.lock().expect("asked").push(checked);
                Ok(may)
            })
            .await
            .expect("answered");

        assert_eq!(*asked.lock().expect("asked"), [project], "may add: {may}");
        let posts: Vec<serde_json::Value> = server
            .received_requests()
            .await
            .unwrap_or_default()
            .iter()
            .filter(|request| request.method.as_str() == "POST")
            .map(|request| request.body_json().expect("json"))
            .collect();
        if may {
            assert_eq!(outcome, FollowUpMade::Made);
            assert_eq!(posts.len(), 1);
            assert_eq!(posts[0]["projectId"], json!(project.to_string()));
        } else {
            assert_eq!(outcome, FollowUpMade::Refused);
            assert!(posts.is_empty(), "nothing asked for");
        }
    }
}
