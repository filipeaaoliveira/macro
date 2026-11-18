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

/// The newest deployment of the commit to the environment is read with
/// its newest status, and the address of the newest status that names one:
/// a later `inactive` status names none.
#[tokio::test]
async fn a_commit_s_deployment_is_read_with_where_it_is_reached() {
    let (server, client) = answering(vec![
        ("200 OK", r#"[{"id":41,"sha":"abc123"}]"#),
        (
            "200 OK",
            r#"[{"state":"inactive","environment_url":""},{"state":"success","environment_url":"https://app.example.com"}]"#,
        ),
    ]);

    let deployments = client
        .list_deployments("token", "macro", "app", Some("abc123"), "production", 1)
        .await
        .unwrap();

    assert_eq!(
        deployments,
        [GithubDeployment {
            id: 41,
            sha: "abc123".to_string(),
            state: Some("inactive".to_string()),
            environment_url: Some("https://app.example.com".to_string()),
        }]
    );
    let requests = server.finish_all();
    assert!(
        requests[0].starts_with(
            "GET /repos/macro/app/deployments?environment=production&per_page=1&sha=abc123 HTTP/1.1"
        ),
        "{}",
        requests[0]
    );
    assert!(
        requests[1]
            .starts_with("GET /repos/macro/app/deployments/41/statuses?per_page=30 HTTP/1.1"),
        "{}",
        requests[1]
    );
}

/// The environment's newest deployments, of any commit, newest first.
#[tokio::test]
async fn an_environment_s_newest_deployments_of_any_commit() {
    let (server, client) = answering(vec![
        ("200 OK", r#"[{"id":43,"sha":"later"},{"id":42,"sha":"merged"}]"#),
        ("200 OK", r#"[{"state":"success"}]"#),
        ("200 OK", r#"[{"state":"error"}]"#),
    ]);

    let deployments = client
        .list_deployments("token", "macro", "app", None, "production", 2)
        .await
        .unwrap();

    let read: Vec<(&str, Option<&str>)> = deployments
        .iter()
        .map(|deployment| (deployment.sha.as_str(), deployment.state.as_deref()))
        .collect();
    assert_eq!(read, [("later", Some("success")), ("merged", Some("error"))]);
    let requests = server.finish_all();
    assert!(
        requests[0]
            .starts_with("GET /repos/macro/app/deployments?environment=production&per_page=2 HTTP/1.1"),
        "{}",
        requests[0]
    );
}

/// A commit not deployed yet has no deployment, and one GitHub made but
/// gave no status has no state.
#[tokio::test]
async fn no_deployment_or_no_status_yet() {
    let (server, client) = answering(vec![("200 OK", "[]")]);
    let none = client
        .list_deployments("token", "macro", "app", Some("abc123"), "production", 1)
        .await
        .unwrap();
    assert_eq!(none, []);
    server.finish_all();

    let (server, client) = answering(vec![
        ("200 OK", r#"[{"id":41,"sha":"abc123"}]"#),
        ("200 OK", "[]"),
    ]);
    let waiting = client
        .list_deployments("token", "macro", "app", Some("abc123"), "production", 1)
        .await
        .unwrap();
    assert_eq!(
        waiting,
        [GithubDeployment {
            id: 41,
            sha: "abc123".to_string(),
            state: None,
            environment_url: None,
        }]
    );
    server.finish_all();
}

/// A repository the user cannot reach is not a missing deployment.
#[tokio::test]
async fn a_repository_out_of_reach_is_an_error() {
    let (server, client) = answering(vec![("404 Not Found", r#"{"message":"Not Found"}"#)]);
    let read = client
        .list_deployments("token", "macro", "app", Some("abc123"), "production", 1)
        .await;
    assert!(
        matches!(read, Err(GithubError::RepositoryUnavailable)),
        "{read:?}"
    );
    server.finish_all();
}
