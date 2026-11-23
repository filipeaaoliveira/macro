use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
use super::GithubJobRerun;
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
async fn a_job_is_rerun_alone() {
    let (server, client) = answering("201 Created", "{}");

    let rerun = client
        .rerun_actions_job("token", "macro", "app", 42)
        .await
        .unwrap();

    assert_eq!(rerun, GithubJobRerun::Started);
    assert!(
        server
            .finish()
            .starts_with("POST /repos/macro/app/actions/jobs/42/rerun HTTP/1.1")
    );
}

#[tokio::test]
async fn a_rerun_github_cannot_make_is_declined_with_its_message() {
    let (server, client) = answering(
        "403 Forbidden",
        r#"{"message":"Unable to retry this workflow run because it was created over a month ago"}"#,
    );

    let rerun = client
        .rerun_actions_job("token", "macro", "app", 42)
        .await
        .unwrap();

    assert_eq!(
        rerun,
        GithubJobRerun::Declined(
            "Unable to retry this workflow run because it was created over a month ago"
                .to_string()
        )
    );
    server.finish();
}

#[tokio::test]
async fn a_missing_repository_is_a_refusal() {
    let (server, client) = answering("404 Not Found", r#"{"message":"Not Found"}"#);

    let rerun = client.rerun_actions_job("token", "macro", "app", 42).await;

    assert!(matches!(rerun, Err(GithubError::RepositoryUnavailable)));
    server.finish();
}
