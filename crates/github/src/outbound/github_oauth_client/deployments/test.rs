use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
use super::GithubDeployment;
use crate::domain::models::GithubError;

fn answering(responses: Vec<(&'static str, &str)>) -> (MockServer, GithubOauthImpl) {
    let server = MockServer::start_many(
        responses
            .into_iter()
            .map(|(status, body)| MockResponse {
                status,
                body: body.to_string(),
            })
            .collect(),
    );
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());
    (server, client)
}

/// The newest deployment of the commit to the environment is read with its
/// newest status.
#[tokio::test]
async fn the_newest_deployment_is_read_with_its_newest_status() {
    let (server, client) = answering(vec![
        ("200 OK", r#"[{"id":41,"sha":"abc123"}]"#),
        (
            "200 OK",
            r#"[{"state":"success","environment_url":"https://app.example.com"}]"#,
        ),
    ]);

    let deployment = client
        .get_latest_deployment("token", "macro", "app", "abc123", "production")
        .await
        .unwrap();

    assert_eq!(
        deployment,
        Some(GithubDeployment {
            id: 41,
            state: Some("success".to_string()),
            environment_url: Some("https://app.example.com".to_string()),
        })
    );
    let requests = server.finish_all();
    assert!(
        requests[0].starts_with(
            "GET /repos/macro/app/deployments?sha=abc123&environment=production&per_page=1 HTTP/1.1"
        ),
        "{}",
        requests[0]
    );
    assert!(
        requests[1].starts_with("GET /repos/macro/app/deployments/41/statuses?per_page=1 HTTP/1.1"),
        "{}",
        requests[1]
    );
}

/// A commit not deployed yet has no deployment, and one GitHub made but
/// gave no status has no state.
#[tokio::test]
async fn no_deployment_or_no_status_yet() {
    let (server, client) = answering(vec![("200 OK", "[]")]);
    let none = client
        .get_latest_deployment("token", "macro", "app", "abc123", "production")
        .await
        .unwrap();
    assert_eq!(none, None);
    server.finish_all();

    let (server, client) = answering(vec![("200 OK", r#"[{"id":41}]"#), ("200 OK", "[]")]);
    let waiting = client
        .get_latest_deployment("token", "macro", "app", "abc123", "production")
        .await
        .unwrap();
    assert_eq!(
        waiting,
        Some(GithubDeployment {
            id: 41,
            state: None,
            environment_url: None,
        })
    );
    server.finish_all();
}

/// A repository the user cannot reach is not a missing deployment.
#[tokio::test]
async fn a_repository_out_of_reach_is_an_error() {
    let (server, client) = answering(vec![("404 Not Found", r#"{"message":"Not Found"}"#)]);
    let read = client
        .get_latest_deployment("token", "macro", "app", "abc123", "production")
        .await;
    assert!(
        matches!(read, Err(GithubError::RepositoryUnavailable)),
        "{read:?}"
    );
    server.finish_all();
}
