use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
use super::GithubBranchCreation;
use crate::domain::models::GithubError;

const BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// GitHub's answers to reading the base commit, making the tree and making
/// the commit, then `branch` to creating the branch.
fn answers(branch: MockResponse) -> MockServer {
    MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","tree":{"sha":"tree0"}}"#
                .to_string(),
        },
        MockResponse {
            status: "201 Created",
            body: r#"{"sha":"tree1"}"#.to_string(),
        },
        MockResponse {
            status: "201 Created",
            body: r#"{"sha":"cccccccccccccccccccccccccccccccccccccccc"}"#.to_string(),
        },
        branch,
    ])
}

/// The files are written over the base commit's tree, in one commit whose
/// parent is the base, and the branch is created at it.
#[tokio::test]
async fn a_branch_is_created_at_a_commit_writing_the_files() {
    let server = answers(MockResponse {
        status: "201 Created",
        body: r#"{"ref":"refs/heads/definition-change/1"}"#.to_string(),
    });
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let created = client
        .create_branch_with_files(
            "token",
            "macro",
            "factory-definition",
            BASE,
            "definition-change/1",
            "Add a flow",
            &[("flows/notes.json", "{\"entries\":[]}")],
        )
        .await
        .unwrap();

    assert_eq!(created, GithubBranchCreation::Created);
    let requests = server.finish_all();
    assert!(
        requests[0].starts_with(&format!(
            "GET /repos/macro/factory-definition/git/commits/{BASE} HTTP/1.1"
        )),
        "{}",
        requests[0]
    );
    assert!(requests[1].starts_with("POST /repos/macro/factory-definition/git/trees HTTP/1.1"));
    assert!(requests[1].contains(r#""base_tree":"tree0""#), "{}", requests[1]);
    assert!(
        requests[1].contains(r#""path":"flows/notes.json""#),
        "{}",
        requests[1]
    );
    assert!(
        requests[1].contains(r#""content":"{\"entries\":[]}""#),
        "{}",
        requests[1]
    );
    assert!(requests[2].starts_with("POST /repos/macro/factory-definition/git/commits HTTP/1.1"));
    assert!(requests[2].contains(r#""tree":"tree1""#), "{}", requests[2]);
    assert!(
        requests[2].contains(&format!(r#""parents":["{BASE}"]"#)),
        "{}",
        requests[2]
    );
    assert!(requests[3].starts_with("POST /repos/macro/factory-definition/git/refs HTTP/1.1"));
    assert!(
        requests[3].contains(r#""ref":"refs/heads/definition-change/1""#),
        "{}",
        requests[3]
    );
    assert!(
        requests[3].contains(r#""sha":"cccccccccccccccccccccccccccccccccccccccc""#),
        "{}",
        requests[3]
    );
}

/// A branch already there is left as it is: GitHub refuses to create it
/// again, which is no failure.
#[tokio::test]
async fn a_branch_already_there_is_left_as_it_is() {
    let server = answers(MockResponse {
        status: "422 Unprocessable Entity",
        body: r#"{"message":"Reference already exists"}"#.to_string(),
    });
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let created = client
        .create_branch_with_files(
            "token",
            "macro",
            "factory-definition",
            BASE,
            "definition-change/1",
            "Add a flow",
            &[("agents.json", "{}")],
        )
        .await
        .unwrap();

    assert_eq!(created, GithubBranchCreation::Exists);
    server.finish_all();
}

/// A user who may not write to the repository is refused, as any read of
/// it is.
#[tokio::test]
async fn a_user_who_may_not_write_is_refused() {
    let server = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"{"tree":{"sha":"tree0"}}"#.to_string(),
        },
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Not Found"}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let refused = client
        .create_branch_with_files(
            "token",
            "macro",
            "factory-definition",
            BASE,
            "definition-change/1",
            "Add a flow",
            &[("agents.json", "{}")],
        )
        .await;

    assert!(
        matches!(refused, Err(GithubError::RepositoryUnavailable)),
        "{refused:?}"
    );
    server.finish_all();
}

/// A branch name git would not take, or a base that is not a commit's id,
/// is never sent.
#[tokio::test]
async fn a_name_git_would_not_take_is_never_sent() {
    let client = GithubOauthImpl::with_api_base_url("http://127.0.0.1:9".to_string());

    for (branch, base) in [("main..x", BASE), ("definition-change/1", "main")] {
        let refused = client
            .create_branch_with_files(
                "token",
                "macro",
                "factory-definition",
                base,
                branch,
                "Add a flow",
                &[("agents.json", "{}")],
            )
            .await;
        assert!(
            matches!(refused, Err(GithubError::Internal(_))),
            "{refused:?}"
        );
    }
}
