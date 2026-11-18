use super::super::GithubOauthImpl;
use super::super::test::{MockResponse, MockServer};
use crate::domain::models::{
    GithubBranchDeletion, GithubError, GithubOpenPullRequest, GithubOpenPullRequestOutcome,
    GithubRepositoryAccess, GithubRequiredCheck, GithubRequiredChecks, GithubWorkflowRunStatus,
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
        r#"{"full_name":"macro/app","default_branch":"trunk","permissions":{"admin":false,"push":true,"pull":true},"delete_branch_on_merge":true}"#,
    );

    let access = client
        .get_repository_access("token", "macro", "app")
        .await
        .unwrap();

    assert_eq!(
        access,
        Some(GithubRepositoryAccess {
            full_name: Some("macro/app".to_string()),
            default_branch: Some("trunk".to_string()),
            push: true,
            delete_branch_on_merge: true,
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

    assert_eq!(access.as_ref().map(|access| access.push), Some(false));
    // A read that does not name the setting reads it as off.
    assert_eq!(
        access.map(|access| access.delete_branch_on_merge),
        Some(false)
    );
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
        r#"[{"number":12,"html_url":"https://github.com/macro/app/pull/12","state":"closed","merged_at":"2026-11-16T10:00:00Z","merge_commit_sha":"def456","head":{"sha":"abc123"}}]"#,
    );

    let found = client
        .find_pull_request_by_head("token", "macro", "app", "factory/run")
        .await
        .unwrap()
        .expect("found");

    assert_eq!(found.number, 12);
    assert!(found.merged);
    assert_eq!(found.head_sha, "abc123");
    assert_eq!(found.merge_commit_sha.as_deref(), Some("def456"));
    assert!(server.finish().starts_with(
        "GET /repos/macro/app/pulls?head=macro%3Afactory%2Frun&state=all&sort=created&direction=desc&per_page=1 HTTP/1.1"
    ));
}

/// An open pull request's merge commit is GitHub's trial merge, never on
/// its base: it is not read.
#[tokio::test]
async fn an_open_pull_request_has_no_merge_commit() {
    let (server, client) = answering(
        "200 OK",
        r#"[{"number":12,"html_url":"https://github.com/macro/app/pull/12","state":"open","merged_at":null,"merge_commit_sha":"def456","head":{"sha":"abc123"}}]"#,
    );

    let found = client
        .find_pull_request_by_head("token", "macro", "app", "factory/run")
        .await
        .unwrap()
        .expect("found");

    assert!(!found.merged);
    assert_eq!(found.merge_commit_sha, None);
    server.finish();
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
        .unwrap()
        .items;
    let suites_request = suites_server.finish();
    let (statuses_server, client) = answering(
        "200 OK",
        r#"{"state":"failure","statuses":[{"context":"ci/lint","state":"failure","target_url":"https://ci.example.com/1"}]}"#,
    );
    let statuses = client
        .list_commit_statuses("token", "macro", "app", "abc")
        .await
        .unwrap()
        .items;
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

    assert_eq!(suites.items.len(), 101);
    assert!(suites.complete);
    let requests = server.finish_all();
    assert!(
        requests[1]
            .starts_with("GET /repos/macro/app/commits/abc/check-suites?per_page=100&page=2 ")
    );
}

/// A commit with more checks than the reader reads is not taken as all
/// there is.
#[tokio::test]
async fn a_list_longer_than_the_pages_read_is_incomplete() {
    let full: Vec<_> = (1..=100)
        .map(|id| serde_json::json!({ "id": id, "status": "completed", "latest_check_runs_count": 1 }))
        .collect();
    let page = serde_json::json!({ "total_count": 2000, "check_suites": full }).to_string();
    let server = MockServer::start_many(
        (0..10)
            .map(|_| MockResponse {
                status: "200 OK",
                body: page.clone(),
            })
            .collect(),
    );
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let suites = client
        .list_check_suites("token", "macro", "app", "abc")
        .await
        .unwrap();

    assert_eq!(suites.items.len(), 1000);
    assert!(!suites.complete);
    server.finish_all();
}

fn required(context: &str, app_id: Option<u64>) -> GithubRequiredCheck {
    GithubRequiredCheck {
        context: context.to_string(),
        app_id,
    }
}

/// The required checks are the branch protection's and the rulesets',
/// once each, each with the app it must come from when the rule names one;
/// GitHub's -1, any source, names none.
#[tokio::test]
async fn required_checks_join_branch_protection_and_rulesets() {
    let server = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"{"name":"main","protected":true,"protection":{"required_status_checks":{"contexts":["test","docs"],"checks":[{"context":"test","app_id":15368},{"context":"docs","app_id":null}]}}}"#.to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: r#"[{"type":"pull_request"},{"type":"required_status_checks","parameters":{"required_status_checks":[{"context":"lint","integration_id":42},{"context":"deploy","integration_id":-1}]}}]"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let checks = client
        .get_required_checks("token", "macro", "app", "release/1.0")
        .await
        .unwrap();

    assert_eq!(
        checks,
        Some(GithubRequiredChecks {
            checks: vec![
                required("deploy", None),
                required("docs", None),
                required("lint", Some(42)),
                required("test", Some(15368)),
            ],
            complete: true,
        })
    );
    let requests = server.finish_all();
    assert!(requests[0].starts_with("GET /repos/macro/app/branches/release%2F1.0 "));
    assert!(
        requests[1]
            .starts_with("GET /repos/macro/app/rules/branches/release%2F1.0?per_page=100&page=1 ")
    );
}

/// The branch's protection still names its checks when its rulesets cannot
/// be read, as a set that may leave some out.
#[tokio::test]
async fn required_checks_keep_what_can_be_read() {
    let server = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"{"name":"main","protected":true,"protection":{"required_status_checks":{"contexts":["test"],"checks":[]}}}"#.to_string(),
        },
        MockResponse {
            status: "403 Forbidden",
            body: r#"{"message":"Resource not accessible by integration"}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let checks = client
        .get_required_checks("token", "macro", "app", "main")
        .await
        .unwrap();

    assert_eq!(
        checks,
        Some(GithubRequiredChecks {
            checks: vec![required("test", None)],
            complete: false,
        })
    );
    server.finish_all();
}

/// A protected branch whose protection is not shown, and a branch whose
/// protection and rules both are refused, require checks no one can name.
#[tokio::test]
async fn required_checks_no_one_can_name_are_unknown() {
    let hidden = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"{"name":"main","protected":true}"#.to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: "[]".to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(hidden.base_url.clone());
    let protected = client
        .get_required_checks("token", "macro", "app", "main")
        .await
        .unwrap();
    hidden.finish_all();
    let refused = MockServer::start_many(vec![
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Not Found"}"#.to_string(),
        },
        MockResponse {
            status: "403 Forbidden",
            body: r#"{"message":"Resource not accessible"}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(refused.base_url.clone());
    let unread = client
        .get_required_checks("token", "macro", "app", "main")
        .await
        .unwrap();
    refused.finish_all();

    assert_eq!(protected, None);
    assert_eq!(unread, None);
}

/// A commit's Actions runs are read page by page for that commit, and keep
/// their status, so a run a concurrency group holds is seen.
#[tokio::test]
async fn workflow_runs_are_read_for_the_head_commit() {
    let server = MockServer::start(MockResponse {
        status: "200 OK",
        body: r#"{"total_count":2,"workflow_runs":[{"status":"pending"},{"status":"completed"}]}"#
            .to_string(),
    });
    let client = GithubOauthImpl::with_api_base_url(server.base_url.clone());

    let runs = client
        .list_workflow_runs("token", "macro", "app", "abc123")
        .await
        .unwrap();

    assert!(runs.complete);
    let statuses: Vec<GithubWorkflowRunStatus> = runs.items.iter().map(|run| run.status).collect();
    assert_eq!(
        statuses,
        [
            GithubWorkflowRunStatus::Waiting,
            GithubWorkflowRunStatus::Completed
        ]
    );
    assert!(
        server
            .finish()
            .starts_with("GET /repos/macro/app/actions/runs?head_sha=abc123&per_page=100&page=1 ")
    );
}

/// A branch is deleted by its ref, its slashes kept as GitHub names it.
#[tokio::test]
async fn delete_branch_deletes_the_branch_s_ref() {
    let (server, client) = answering("204 No Content", "");

    let deleted = client
        .delete_branch("token", "macro", "app", "factory/run")
        .await
        .unwrap();

    assert_eq!(deleted, GithubBranchDeletion::Deleted);
    assert!(
        server
            .finish()
            .starts_with("DELETE /repos/macro/app/git/refs/heads/factory/run HTTP/1.1")
    );
}

/// A branch already gone, as one GitHub deleted on merge, is not deleted
/// again, and is no error.
#[tokio::test]
async fn delete_branch_of_a_branch_already_gone_deletes_nothing() {
    let (server, client) = answering(
        "422 Unprocessable Entity",
        r#"{"message":"Reference does not exist","documentation_url":"https://docs.github.com/rest/git/refs#delete-a-reference","status":"422"}"#,
    );

    let deleted = client
        .delete_branch("token", "macro", "app", "factory/run")
        .await
        .unwrap();

    assert_eq!(deleted, GithubBranchDeletion::Missing);
    server.finish();
}

/// GitHub refusing the deletion, as of a protected branch or by a user who
/// may not push, is an error, not a branch already gone.
#[tokio::test]
async fn delete_branch_refused_is_an_error() {
    let (protected, client) = answering(
        "422 Unprocessable Entity",
        r#"{"message":"Cannot delete this protected branch"}"#,
    );
    let refused = client
        .delete_branch("token", "macro", "app", "factory/run")
        .await;
    assert!(
        matches!(refused, Err(GithubError::Internal(_))),
        "{refused:?}"
    );
    protected.finish();

    let (forbidden, client) = answering("403 Forbidden", r#"{"message":"Forbidden"}"#);
    let refused = client
        .delete_branch("token", "macro", "app", "factory/run")
        .await;
    assert!(
        matches!(refused, Err(GithubError::Forbidden)),
        "{refused:?}"
    );
    forbidden.finish();
}

/// Each part of a branch's name is encoded, its slashes kept.
#[tokio::test]
async fn delete_branch_encodes_each_part_of_the_name() {
    let (server, client) = answering("204 No Content", "");

    client
        .delete_branch("token", "macro", "app", "factory/ü#1")
        .await
        .unwrap();

    assert!(
        server
            .finish()
            .starts_with("DELETE /repos/macro/app/git/refs/heads/factory/%C3%BC%231 HTTP/1.1")
    );

    // An encoded dot stays text: GitHub never reads it as `..`.
    let (server, client) = answering("204 No Content", "");
    client
        .delete_branch("token", "macro", "app", "factory/%2e%2e")
        .await
        .unwrap();
    assert!(
        server
            .finish()
            .starts_with("DELETE /repos/macro/app/git/refs/heads/factory/%252e%252e HTTP/1.1")
    );
}

/// An owner or repository name GitHub would not take is refused before any
/// request, as `..` would move the deletion off the repository.
#[tokio::test]
async fn delete_branch_refuses_an_owner_or_repository_github_would_not_take() {
    // Nothing listens here: a request sent would fail as an outage.
    let client = GithubOauthImpl::with_api_base_url("http://127.0.0.1:9".to_string());

    for (owner, repo) in [
        ("..", "app"),
        (".", "app"),
        ("macro", ".."),
        ("macro", "."),
        ("", "app"),
        ("macro", ""),
        ("macro/x", "app"),
        ("macro", "app?x"),
        ("macro", "app#x"),
        ("macro", "a%2e"),
    ] {
        let refused = client
            .delete_branch("token", owner, repo, "factory/run")
            .await;
        assert!(
            matches!(refused, Err(GithubError::Internal(_))),
            "{owner:?}/{repo:?}: {refused:?}"
        );
    }
}

/// A name git would not take for a branch is refused before any request:
/// `..` parts would otherwise move the deletion onto the repository.
#[tokio::test]
async fn delete_branch_refuses_a_name_git_would_not_take() {
    // Nothing listens here: a request sent would fail as an outage.
    let client = GithubOauthImpl::with_api_base_url("http://127.0.0.1:9".to_string());

    for branch in [
        "../../..",
        "factory/..",
        "factory/./run",
        "factory//run",
        "factory/run/",
        "factory/.hidden",
        "factory/run.lock",
        "factory/run.",
        "-factory",
        "factory/a..b",
        "factory/@{1}",
        "@",
        "",
        "factory/a b",
        "factory/a\tb",
        "factory/a~1",
        "factory/a^",
        "factory/a:b",
        "factory/a?",
        "factory/a*",
        "factory/a[b",
        "factory\\run",
    ] {
        let refused = client.delete_branch("token", "macro", "app", branch).await;
        assert!(
            matches!(refused, Err(GithubError::Internal(_))),
            "{branch:?}: {refused:?}"
        );
    }
}

/// An open pull request based on a branch is found by its base.
#[tokio::test]
async fn an_open_pull_request_onto_a_branch_is_found() {
    let (based, client) = answering("200 OK", r#"[{"number":14}]"#);
    let found = client
        .has_open_pull_request_onto("token", "macro", "app", "factory/run")
        .await
        .unwrap();
    assert!(found);
    assert!(
        based
            .finish()
            .starts_with("GET /repos/macro/app/pulls?base=factory%2Frun&state=open&per_page=1 ")
    );

    let (none, client) = answering("200 OK", "[]");
    let found = client
        .has_open_pull_request_onto("token", "macro", "app", "factory/run")
        .await
        .unwrap();
    assert!(!found);
    none.finish();
}

/// An owner or repository name GitHub would not take is refused before the
/// pull requests onto a branch are listed.
#[tokio::test]
async fn pull_requests_onto_a_branch_refuse_an_owner_or_repository_github_would_not_take() {
    // Nothing listens here: a request sent would fail as an outage.
    let client = GithubOauthImpl::with_api_base_url("http://127.0.0.1:9".to_string());

    for (owner, repo) in [
        ("..", "app"),
        ("macro", ".."),
        ("macro/x", "app"),
        ("macro", ""),
    ] {
        let refused = client
            .has_open_pull_request_onto("token", owner, repo, "factory/run")
            .await;
        assert!(
            matches!(refused, Err(GithubError::Internal(_))),
            "{owner:?}/{repo:?}: {refused:?}"
        );
    }
}

/// A branch a ruleset asks a pull request for, or whose protection asks
/// for reviews, takes changes only through one; neither, and it takes
/// pushes; a protection refused to the caller says nothing.
#[tokio::test]
async fn a_branch_requires_a_pull_request_by_ruleset_or_protection() {
    let by_ruleset = MockServer::start_many(vec![MockResponse {
        status: "200 OK",
        body: r#"[{"type":"pull_request","parameters":{}}]"#.to_string(),
    }]);
    let client = GithubOauthImpl::with_api_base_url(by_ruleset.base_url.clone());
    let ruleset = client
        .get_requires_pull_request("token", "macro", "definition", "main")
        .await
        .unwrap();
    by_ruleset.finish_all();

    let by_protection = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: "[]".to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: r#"{"required_approving_review_count":1}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(by_protection.base_url.clone());
    let protection = client
        .get_requires_pull_request("token", "macro", "definition", "main")
        .await
        .unwrap();
    let requests = by_protection.finish_all();

    let neither = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body:
                r#"[{"type":"required_status_checks","parameters":{"required_status_checks":[]}}]"#
                    .to_string(),
        },
        MockResponse {
            status: "404 Not Found",
            body: r#"{"message":"Not Found"}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(neither.base_url.clone());
    let unprotected = client
        .get_requires_pull_request("token", "macro", "definition", "main")
        .await
        .unwrap();
    neither.finish_all();

    let refused = MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: "[]".to_string(),
        },
        MockResponse {
            status: "403 Forbidden",
            body: r#"{"message":"Must have admin rights to Repository."}"#.to_string(),
        },
    ]);
    let client = GithubOauthImpl::with_api_base_url(refused.base_url.clone());
    let unknown = client
        .get_requires_pull_request("token", "macro", "definition", "main")
        .await
        .unwrap();
    refused.finish_all();

    assert_eq!(ruleset, Some(true));
    assert_eq!(protection, Some(true));
    assert!(requests[1].starts_with(
        "GET /repos/macro/definition/branches/main/protection/required_pull_request_reviews "
    ));
    assert_eq!(unprotected, Some(false));
    assert_eq!(unknown, None);
}
