use messages::domain::ports::MessageTimelineQuery;
use reqwest::StatusCode;
use serde_json::json;
use uuid::Uuid;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::ReadMessagesError;
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
