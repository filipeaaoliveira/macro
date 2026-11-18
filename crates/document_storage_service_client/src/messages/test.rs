use messages::domain::ports::MessageTimelineQuery;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CommentPosted, NewDocumentComment, PostCommentError, ReadMessagesError};
use crate::DocumentStorageServiceClient;

const DOCUMENT: Uuid = Uuid::from_u128(0x7a5c);

/// A page is read as the user, with the internal key and the query as its
/// selection.
#[tokio::test]
async fn a_document_s_discussions_are_read_as_the_user() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/messages/document/{DOCUMENT}")))
        .and(header("x-document-storage-service-auth-key", "internal-key"))
        .and(header(
            "x-document-storage-service-user-id",
            "macro|julia@example.com",
        ))
        .and(query_param("selection", r#"{"cursor":null,"direction":"older","around":null,"ids":[],"anchored":null,"include_deleted_threads":false,"activity_after":null,"activity_before":null,"limit":5}"#))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [],
            "next_cursor": null,
            "previous_cursor": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let page = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .document_messages(
            DOCUMENT,
            "macro|julia@example.com",
            &MessageTimelineQuery {
                limit: Some(5),
                ..MessageTimelineQuery::default()
            },
        )
        .await
        .expect("read");
    assert!(page.items.is_empty());
}

/// A refusal or a missing document is not there for the user; any other
/// failure is worth trying again.
#[tokio::test]
async fn a_refusal_is_not_there_and_an_outage_is_unavailable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/messages/document/{DOCUMENT}/threads/{}",
            Uuid::nil()
        )))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let client = DocumentStorageServiceClient::new("internal-key".into(), server.uri());
    let missing = client
        .document_thread(DOCUMENT, "macro|julia@example.com", Uuid::nil())
        .await;
    assert!(matches!(
        missing,
        Err(ReadMessagesError::NotThere(StatusCode::NOT_FOUND))
    ));

    let down = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&down)
        .await;
    let client = DocumentStorageServiceClient::new("internal-key".into(), down.uri());
    let failed = client
        .document_thread(DOCUMENT, "macro|julia@example.com", Uuid::nil())
        .await;
    assert!(matches!(failed, Err(ReadMessagesError::Unavailable(_))));
}

/// A comment is posted as the user, with the internal key, its id and the
/// thread it replies to.
#[tokio::test]
async fn a_comment_is_posted_as_the_user_with_its_id_and_thread() {
    let server = MockServer::start().await;
    let id = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0001);
    let root = Uuid::from_u128(0x0193_0000_0000_7000_8000_0000_0000_0002);
    Mock::given(method("POST"))
        .and(path(format!("/messages/document/{DOCUMENT}")))
        .and(header(
            "x-document-storage-service-auth-key",
            "internal-key",
        ))
        .and(header(
            "x-document-storage-service-user-id",
            "macro|julia@example.com",
        ))
        .and(body_json(json!({
            "id": id,
            "content": "The run ended.",
            "thread_id": root
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let posted = DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .post_document_comment(
            DOCUMENT,
            "macro|julia@example.com",
            &NewDocumentComment {
                id,
                content: "The run ended.",
                thread_id: Some(root),
            },
        )
        .await
        .expect("posted");
    assert_eq!(posted, CommentPosted::Posted);
}

/// A conflict is a comment posted before; a refusal or a missing document
/// or thread is not there; another client error is refused; anything else
/// is worth trying again.
#[tokio::test]
async fn a_conflict_was_posted_before_and_a_missing_thread_is_not_there() {
    assert_eq!(
        posted_when_answered(409).await.expect("posted before"),
        CommentPosted::AlreadyPosted
    );
    assert_eq!(
        posted_when_answered(404).await.expect("not there"),
        CommentPosted::NotThere
    );
    assert_eq!(
        posted_when_answered(403).await.expect("not there"),
        CommentPosted::NotThere
    );
    assert!(matches!(
        posted_when_answered(400).await,
        Err(PostCommentError::Refused(StatusCode::BAD_REQUEST, _))
    ));
    assert!(matches!(
        posted_when_answered(502).await,
        Err(PostCommentError::Unavailable(_))
    ));
}

/// Post a comment to a document storage that answers `status`.
async fn posted_when_answered(status: u16) -> Result<CommentPosted, PostCommentError> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(status))
        .mount(&server)
        .await;
    let comment = NewDocumentComment {
        id: Uuid::nil(),
        content: "The run started.",
        thread_id: None,
    };
    DocumentStorageServiceClient::new("internal-key".into(), server.uri())
        .post_document_comment(DOCUMENT, "macro|julia@example.com", &comment)
        .await
}
