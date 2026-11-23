use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
use super::GithubIssueComment;
use crate::domain::models::GithubError;

fn answering(status: &'static str, body: &str) -> (MockServer, GithubOauthImpl) {
    let server = MockServer::start(MockResponse {
        status,
        body: body.to_string(),
    });
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());
    (server, client)
}

#[tokio::test]
async fn a_pull_request_s_description_is_read_and_none_when_empty() {
    let (server, client) = answering(
        "200 OK",
        r#"{"number":7,"body":"Why it changes","user":{"id":42,"login":"julia"}}"#,
    );
    let body = client
        .pull_request_text("token", "macro", "app", 7)
        .await
        .unwrap();
    assert_eq!(body.body.as_deref(), Some("Why it changes"));
    assert_eq!(body.author_id, Some(42));
    assert!(
        server
            .finish()
            .starts_with("GET /repos/macro/app/pulls/7 HTTP/1.1")
    );

    let (server, client) = answering("200 OK", r#"{"number":7,"body":null}"#);
    let body = client
        .pull_request_text("token", "macro", "app", 7)
        .await
        .unwrap();
    assert_eq!(body.body, None);
    assert_eq!(body.author_id, None);
    server.finish();
}

#[tokio::test]
async fn a_description_is_replaced_with_a_patch_of_its_body() {
    let (server, client) = answering("200 OK", r#"{"number":7}"#);

    client
        .edit_pull_request_body("token", "macro", "app", 7, "New text")
        .await
        .unwrap();

    let request = server.finish();
    assert!(request.starts_with("PATCH /repos/macro/app/pulls/7 HTTP/1.1"));
    assert!(request.ends_with(r#"{"body":"New text"}"#));
}

#[tokio::test]
async fn comments_are_listed_added_and_edited() {
    let (server, client) = answering(
        "200 OK",
        r#"[{"id":11,"body":"First","user":{"id":42}},{"id":12,"body":null,"user":null}]"#,
    );
    let listed = client
        .list_issue_comments("token", "macro", "app", 7)
        .await
        .unwrap();
    assert_eq!(
        listed.items,
        vec![
            GithubIssueComment {
                id: 11,
                body: "First".to_string(),
                author_id: Some(42),
            },
            GithubIssueComment {
                id: 12,
                body: String::new(),
                author_id: None,
            },
        ]
    );
    assert!(listed.complete);
    assert!(
        server
            .finish()
            .starts_with("GET /repos/macro/app/issues/7/comments?per_page=100&page=1 HTTP/1.1")
    );

    let (server, client) = answering("201 Created", r#"{"id":13}"#);
    client
        .create_issue_comment("token", "macro", "app", 7, "Part 1")
        .await
        .unwrap();
    let request = server.finish();
    assert!(request.starts_with("POST /repos/macro/app/issues/7/comments HTTP/1.1"));
    assert!(request.ends_with(r#"{"body":"Part 1"}"#));

    let (server, client) = answering("200 OK", r#"{"id":13}"#);
    client
        .edit_issue_comment("token", "macro", "app", 13, "Part 1 again")
        .await
        .unwrap();
    let request = server.finish();
    assert!(request.starts_with("PATCH /repos/macro/app/issues/comments/13 HTTP/1.1"));
    assert!(request.ends_with(r#"{"body":"Part 1 again"}"#));
}

/// A write GitHub refuses is a refusal, not an outage to try again.
#[tokio::test]
async fn a_refused_edit_is_forbidden() {
    let (server, client) = answering("403 Forbidden", r#"{"message":"Resource not accessible"}"#);

    let edited = client
        .edit_pull_request_body("token", "macro", "app", 7, "New text")
        .await;

    assert!(matches!(edited, Err(GithubError::Forbidden)));
    server.finish();
}

/// Comments past one page are read from the next.
#[tokio::test]
async fn comments_past_one_page_are_read() {
    let full: Vec<String> = (1..=100)
        .map(|id| format!(r#"{{"id":{id},"body":"c","user":{{"id":42}}}}"#))
        .collect();
    let server = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: format!("[{}]", full.join(",")),
        },
        MockResponse {
            status: "200 OK",
            body: r#"[{"id":101,"body":"last","user":{"id":42}}]"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let listed = client
        .list_issue_comments("token", "macro", "app", 7)
        .await
        .unwrap();

    assert_eq!(listed.items.len(), 101);
    assert_eq!(listed.items[100].body, "last");
    assert!(listed.complete);
    let requests = server.finish_all();
    assert!(requests[1].contains("per_page=100&page=2"));
}

/// Editing a comment someone deleted is a refusal, not an outage.
#[tokio::test]
async fn editing_a_deleted_comment_is_refused() {
    let (server, client) = answering("404 Not Found", r#"{"message":"Not Found"}"#);

    let edited = client
        .edit_issue_comment("token", "macro", "app", 13, "Part 1")
        .await;

    assert!(matches!(edited, Err(GithubError::RepositoryUnavailable)));
    server.finish();
}

/// The account a token acts as is read from `/user`.
#[tokio::test]
async fn a_token_s_account_is_its_user() {
    let (server, client) = answering("200 OK", r#"{"id":42,"login":"julia"}"#);

    let account = client.token_account_id("token").await.unwrap();

    assert_eq!(account, 42);
    assert!(server.finish().starts_with("GET /user HTTP/1.1"));
}
