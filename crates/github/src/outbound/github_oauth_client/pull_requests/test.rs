use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
use crate::domain::models::{
    GithubError, GithubOpenPullRequest, GithubOpenPullRequestOutcome, GithubRepositoryAccess,
};

fn answering(status: &'static str, body: &str) -> (MockServer, GithubOauthImpl) {
    let server = MockServer::start(MockResponse {
        status,
        body: body.to_string(),
    });
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());
    (server, client)
}

fn open_request() -> GithubOpenPullRequest {
    GithubOpenPullRequest {
        head: "factory/run".to_string(),
        base: "main".to_string(),
        title: "Collapse repeated dashes".to_string(),
        body: "Why".to_string(),
    }
}

#[tokio::test]
async fn repository_access_reads_the_default_branch_and_push_permission() {
    let (server, client) = answering(
        "200 OK",
        r#"{"default_branch":"trunk","permissions":{"admin":false,"push":true,"pull":true}}"#,
    );

    let access = client
        .get_repository_access("token", "macro", "app")
        .await
        .unwrap();

    assert_eq!(
        access,
        Some(GithubRepositoryAccess {
            default_branch: Some("trunk".to_string()),
            push: true,
        })
    );
    assert!(server.finish().starts_with("GET /repos/macro/app HTTP/1.1"));
}

/// A repository read with no permissions, as a token that only reads
/// public data sees one, cannot be pushed to.
#[tokio::test]
async fn repository_access_without_permissions_cannot_push() {
    let (server, client) = answering("200 OK", r#"{"default_branch":"main"}"#);

    let access = client
        .get_repository_access("token", "macro", "app")
        .await
        .unwrap();

    assert_eq!(access.map(|access| access.push), Some(false));
    server.finish();
}

#[tokio::test]
async fn a_repository_the_user_cannot_see_is_none() {
    let (server, client) = answering("404 Not Found", r#"{"message":"Not Found"}"#);

    let access = client
        .get_repository_access("token", "macro", "app")
        .await
        .unwrap();

    assert_eq!(access, None);
    server.finish();
}

/// A token GitHub no longer takes, and a refusal that is not throttling,
/// are told apart from an outage, so a caller does not retry them.
#[tokio::test]
async fn refusals_are_told_apart_from_throttling() {
    let (expired, client) = answering("401 Unauthorized", r#"{"message":"Bad credentials"}"#);
    let unauthorized = client
        .find_pull_request_by_head("token", "macro", "app", "factory/run")
        .await;
    expired.finish();
    let (refused, client) = answering("403 Forbidden", r#"{"message":"Resource not accessible"}"#);
    let forbidden = client
        .list_check_suites("token", "macro", "app", "abc")
        .await;
    refused.finish();
    let (throttled, client) = answering(
        "403 Forbidden",
        r#"{"message":"You have exceeded a secondary rate limit."}"#,
    );
    let rate_limited = client
        .list_commit_statuses("token", "macro", "app", "abc")
        .await;
    throttled.finish();

    assert!(matches!(
        unauthorized,
        Err(GithubError::ReauthenticationRequired)
    ));
    assert!(matches!(forbidden, Err(GithubError::Forbidden)));
    assert!(matches!(rate_limited, Err(GithubError::Upstream(_))));
}

#[tokio::test]
async fn find_pull_request_by_head_reads_the_newest_of_the_branch_with_its_head() {
    let (server, client) = answering(
        "200 OK",
        r#"[{"number":12,"html_url":"https://github.com/macro/app/pull/12","state":"closed","merged_at":"2026-11-16T10:00:00Z","head":{"sha":"abc123"}}]"#,
    );

    let found = client
        .find_pull_request_by_head("token", "macro", "app", "factory/run")
        .await
        .unwrap()
        .expect("found");

    assert_eq!(found.number, 12);
    assert!(found.merged);
    assert_eq!(found.head_sha, "abc123");
    assert!(server.finish().starts_with(
        "GET /repos/macro/app/pulls?head=macro%3Afactory%2Frun&state=all&sort=created&direction=desc&per_page=1 HTTP/1.1"
    ));
}

#[tokio::test]
async fn find_pull_request_by_head_with_none_is_none() {
    let (server, client) = answering("200 OK", "[]");

    let found = client
        .find_pull_request_by_head("token", "macro", "app", "factory/run")
        .await
        .unwrap();

    assert_eq!(found, None);
    server.finish();
}

/// A suite reads its app and how many runs it holds; a commit status reads
/// its context and state.
#[tokio::test]
async fn check_suites_and_statuses_read_what_ci_waits_on() {
    let (suites_server, client) = answering(
        "200 OK",
        r#"{"total_count":2,"check_suites":[{"id":1,"status":"completed","conclusion":"success","latest_check_runs_count":1,"app":{"slug":"github-actions"}},{"id":2,"status":"queued","conclusion":null,"latest_check_runs_count":0,"app":{"slug":"other"}}]}"#,
    );
    let suites = client
        .list_check_suites("token", "macro", "app", "abc")
        .await
        .unwrap();
    let suites_request = suites_server.finish();
    let (statuses_server, client) = answering(
        "200 OK",
        r#"{"state":"failure","statuses":[{"context":"ci/lint","state":"failure","target_url":"https://ci.example.com/1"}]}"#,
    );
    let statuses = client
        .list_commit_statuses("token", "macro", "app", "abc")
        .await
        .unwrap();
    let statuses_request = statuses_server.finish();

    assert_eq!(suites.len(), 2);
    assert_eq!(suites[0].app, "github-actions");
    assert_eq!(suites[1].check_runs, 0);
    assert!(
        suites_request
            .starts_with("GET /repos/macro/app/commits/abc/check-suites?per_page=100&page=1 ")
    );
    assert_eq!(statuses[0].context, "ci/lint");
    assert_eq!(statuses[0].state, "failure");
    assert!(
        statuses_request
            .starts_with("GET /repos/macro/app/commits/abc/status?per_page=100&page=1 ")
    );
}

#[tokio::test]
async fn open_pull_request_reports_its_number() {
    let (server, client) = answering("201 Created", r#"{"number":13}"#);

    let outcome = client
        .open_pull_request("token", "macro", "app", &open_request())
        .await
        .unwrap();

    assert_eq!(outcome, GithubOpenPullRequestOutcome::Opened(13));
    let request = server.finish();
    assert!(request.starts_with("POST /repos/macro/app/pulls HTTP/1.1"));
    assert!(request.ends_with(
        r#"{"title":"Collapse repeated dashes","body":"Why","head":"factory/run","base":"main"}"#
    ));
}

#[tokio::test]
async fn open_pull_request_reports_github_s_reason_for_declining() {
    let (server, client) = answering(
        "422 Unprocessable Entity",
        r#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"A pull request already exists for macro:factory/run."}]}"#,
    );

    let outcome = client
        .open_pull_request("token", "macro", "app", &open_request())
        .await
        .unwrap();

    assert_eq!(
        outcome,
        GithubOpenPullRequestOutcome::Rejected(
            "A pull request already exists for macro:factory/run.".to_string()
        )
    );
    server.finish();
}

/// A user who may not reach the repository is refused for good, not told
/// GitHub declined the pull request.
#[tokio::test]
async fn open_pull_request_without_access_is_a_refusal() {
    let (server, client) = answering("404 Not Found", r#"{"message":"Not Found"}"#);

    let outcome = client
        .open_pull_request("token", "macro", "app", &open_request())
        .await;

    assert!(matches!(outcome, Err(GithubError::RepositoryUnavailable)));
    server.finish();
}

/// A 403 that says the token's quota is spent is throttling, tried again,
/// not a refusal.
#[tokio::test]
async fn a_403_with_no_quota_left_is_throttling() {
    let server = MockServer::start_with_headers(vec![(
        MockResponse {
            status: "403 Forbidden",
            body: r#"{"message":"API rate limit exceeded"}"#.to_string(),
        },
        "x-ratelimit-remaining: 0\r\nx-ratelimit-reset: 4102444800\r\n",
    )]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let read = client
        .list_check_suites("token", "macro", "app", "abc")
        .await;

    assert!(matches!(read, Err(GithubError::Upstream(_))));
    server.finish();
}

/// A commit with more check suites than one page holds is read to its
/// last page.
#[tokio::test]
async fn every_page_of_check_suites_is_read() {
    let suite = |id: u64| {
        serde_json::json!({
            "id": id, "status": "completed", "conclusion": "success",
            "latest_check_runs_count": 1, "app": { "slug": "ci" }
        })
    };
    let full: Vec<_> = (1..=100).map(suite).collect();
    let server = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: serde_json::json!({ "total_count": 101, "check_suites": full }).to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: serde_json::json!({ "total_count": 101, "check_suites": [suite(101)] })
                .to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let suites = client
        .list_check_suites("token", "macro", "app", "abc")
        .await
        .unwrap();

    assert_eq!(suites.len(), 101);
    let requests = server.finish_all();
    assert!(
        requests[1]
            .starts_with("GET /repos/macro/app/commits/abc/check-suites?per_page=100&page=2 ")
    );
}

/// The required checks are the branch protection's and the rulesets',
/// once each.
#[tokio::test]
async fn required_checks_join_branch_protection_and_rulesets() {
    let server = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"{"name":"main","protected":true,"protection":{"required_status_checks":{"contexts":["test"],"checks":[{"context":"test","app_id":15368}]}}}"#.to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: r#"[{"type":"pull_request"},{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"lint"}]}}]"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let required = client
        .get_required_checks("token", "macro", "app", "main")
        .await
        .unwrap();

    assert_eq!(required, Some(vec!["lint".to_string(), "test".to_string()]));
    server.finish_all();
}

/// A user who may not read the branch's rules cannot be told which checks
/// are required.
#[tokio::test]
async fn required_checks_the_user_may_not_read_are_unknown() {
    let server = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"{"name":"main","protected":true}"#.to_string(),
        },
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Not Found"}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let required = client
        .get_required_checks("token", "macro", "app", "main")
        .await
        .unwrap();

    assert_eq!(required, None);
    server.finish_all();
}
