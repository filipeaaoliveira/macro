use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use factory_records::domain::models::RunId;
use github::domain::models::GithubError;
use github::outbound::github_oauth_client::GithubOauthImpl;
use macro_user_id::user_id::MacroUserIdStr;
use upstream_error::{UpstreamError, UpstreamFailure};

use wiremock::matchers::{body_partial_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{GithubPullRequests, GithubPullRequestsErr, OwnerTokens};
use crate::domain::models::{BranchDeletion, CodeRepository, HeadChecks, Merging};
use crate::domain::ports::{PullRequests, Retryable};

/// An owner's linked token, or none, counting the reads of it.
#[derive(Clone, Default)]
struct Tokens {
    linked: Option<&'static str>,
    reads: Arc<AtomicUsize>,
}

impl OwnerTokens for Tokens {
    async fn token(
        &self,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<Option<String>, rootcause::Report> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(self.linked.map(str::to_string))
    }
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

fn repository() -> CodeRepository {
    CodeRepository::parse("https://github.com/macro/app").expect("valid")
}

/// An owner with no link reaches no repository, and every other call fails
/// for good without asking GitHub.
#[tokio::test]
async fn an_owner_with_no_link_fails_for_good() {
    let tokens = Tokens::default();
    let github = GithubPullRequests::new(tokens.clone());

    let access = github
        .repository_access(&julia(), &repository())
        .await
        .expect("no link reaches nothing");
    let found = github.find(&julia(), &repository(), "factory/run").await;
    let checked = github
        .find_with_checks(&julia(), &repository(), "factory/run", false)
        .await;

    assert_eq!(access, None);
    let Err(error) = found else {
        panic!("finding needs a link");
    };
    assert!(matches!(error, GithubPullRequestsErr::NotLinked));
    assert!(!error.retryable());
    assert!(matches!(checked, Err(GithubPullRequestsErr::NotLinked)));
    assert_eq!(tokens.reads.load(Ordering::SeqCst), 3);
}

/// The branch deleted is the Run's, named from its id: no other name
/// reaches GitHub.
#[tokio::test]
async fn a_run_s_branch_is_deleted_by_its_id() {
    let run_id = RunId::generate();
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/repos/macro/app/git/refs/heads/factory/{run_id}"
        )))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let github = GithubPullRequests::with_client(
        Tokens {
            linked: Some("token"),
            ..Tokens::default()
        },
        GithubOauthImpl::with_api_base_url(server.uri()),
    );

    let deletion = github
        .delete_run_branch(&julia(), &repository(), run_id)
        .await
        .expect("deleted");

    assert_eq!(deletion, BranchDeletion::Deleted);
    server.verify().await;
}

/// A Run merges its pull request on GitHub itself, with its owner's token
/// and at the head its checks passed, never through Macro's GitHub merge
/// route, which refuses to merge under a Run that goes on.
#[tokio::test]
async fn a_run_merges_its_pull_request_on_github_with_its_owner_s_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/macro/app"))
        .and(header("Authorization", "Bearer token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "allow_merge_commit": false,
            "allow_squash_merge": true,
            "allow_rebase_merge": false,
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/repos/macro/app/pulls/7/merge"))
        .and(header("Authorization", "Bearer token"))
        .and(body_partial_json(serde_json::json!({
            "merge_method": "squash",
            "sha": "abc123",
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": "def456",
            "message": "Pull Request successfully merged",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let github = GithubPullRequests::with_client(
        Tokens {
            linked: Some("token"),
            ..Tokens::default()
        },
        GithubOauthImpl::with_api_base_url(server.uri()),
    );

    let merging = github
        .merge(&julia(), &repository(), 7, "abc123")
        .await
        .expect("merged");

    assert_eq!(merging, Merging::Merged);
    server.verify().await;
}

/// A Run on an Enterprise Managed User's repository, whose owner ends
/// `_shortcode`, still merges its pull request on GitHub.
#[tokio::test]
async fn a_run_merges_a_pull_request_of_an_owner_with_an_underscore() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/acme_labs/widgets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "allow_merge_commit": true,
            "allow_squash_merge": false,
            "allow_rebase_merge": false,
        })))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/repos/acme_labs/widgets/pulls/7/merge"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "sha": "def456",
            "message": "Pull Request successfully merged",
        })))
        .expect(1)
        .mount(&server)
        .await;
    let github = GithubPullRequests::with_client(
        Tokens {
            linked: Some("token"),
            ..Tokens::default()
        },
        GithubOauthImpl::with_api_base_url(server.uri()),
    );
    let repository = CodeRepository::parse("https://github.com/acme_labs/widgets").expect("valid");

    let merging = github
        .merge(&julia(), &repository, 7, "abc123")
        .await
        .expect("merged");

    assert_eq!(merging, Merging::Merged);
    server.verify().await;
}

/// GitHub failing or throttling, and a token that could not be read, are
/// tried again; a token GitHub no longer takes, a refusal, and a repository
/// out of reach are not.
#[test]
fn only_outages_are_tried_again() {
    let outage = GithubPullRequestsErr::Github(GithubError::Upstream(UpstreamError {
        service: "GitHub",
        failure: UpstreamFailure::Unavailable,
    }));
    let token = GithubPullRequestsErr::Token(rootcause::report!("auth service down"));
    let permanent = [
        GithubError::ReauthenticationRequired,
        GithubError::Forbidden,
        GithubError::RepositoryUnavailable,
        GithubError::Internal(anyhow::anyhow!("unexpected answer")),
    ];

    assert!(outage.retryable());
    assert!(token.retryable());
    for error in permanent {
        let error = GithubPullRequestsErr::Github(error);
        assert!(!error.retryable(), "{error}");
    }
}

/// A linked token is read again on each call, never kept, so relinking or
/// unlinking shows at once. GitHub here is a closed port, which fails each
/// call as an outage after its token was read.
#[tokio::test]
async fn a_linked_token_is_read_on_each_call() {
    let tokens = Tokens {
        linked: Some("token"),
        ..Tokens::default()
    };
    let github = GithubPullRequests::with_client(
        tokens.clone(),
        GithubOauthImpl::with_api_base_url("http://127.0.0.1:9".to_string()),
    );

    for _ in 0..3 {
        let found = github.find(&julia(), &repository(), "factory/run").await;
        assert!(matches!(
            found,
            Err(GithubPullRequestsErr::Github(GithubError::Upstream(_)))
        ));
    }

    assert_eq!(tokens.reads.load(Ordering::SeqCst), 3);
}

const EMPTY_ACTIONS_SUITE: &str = r#"[{"id":1,"status":"queued","conclusion":null,"latest_check_runs_count":0,"app":{"slug":"github-actions"}}]"#;

/// GitHub for one CI read of a head: its pull request, the suites `suites`
/// (GitHub's JSON list), no statuses, and `runs` answering for its workflow
/// runs.
async fn github_with(suites: &str, runs: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/macro/app/pulls"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"[{"number":7,"html_url":"https://github.com/macro/app/pull/7","state":"open","merged_at":null,"head":{"sha":"abc123"}}]"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/macro/app/commits/abc123/check-suites"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(format!(r#"{{"total_count":1,"check_suites":{suites}}}"#)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/macro/app/commits/abc123/status"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"state":"pending","statuses":[]}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/macro/app/actions/runs"))
        .and(query_param("head_sha", "abc123"))
        .respond_with(runs)
        .mount(&server)
        .await;
    server
}

fn runs(statuses: &[&str]) -> ResponseTemplate {
    let runs: Vec<_> = statuses
        .iter()
        .map(|status| serde_json::json!({ "status": status }))
        .collect();
    ResponseTemplate::new(200)
        .set_body_json(serde_json::json!({ "total_count": runs.len(), "workflow_runs": runs }))
}

/// One CI read of the head on `server`.
async fn read_checks(server: &MockServer) -> Result<HeadChecks, GithubPullRequestsErr> {
    let github = GithubPullRequests::with_client(
        Tokens {
            linked: Some("token"),
            ..Tokens::default()
        },
        GithubOauthImpl::with_api_base_url(server.uri()),
    );
    let (_, checks) = github
        .find_with_checks(&julia(), &repository(), "factory/run", false)
        .await?
        .expect("a pull request");
    Ok(checks)
}

/// An Actions run still pending holds CI as GitHub Actions', before
/// GitHub made any suite for it as well as with an empty one; runs that all
/// completed hold nothing.
#[tokio::test]
async fn a_pending_actions_run_holds_with_or_without_its_suite() {
    let no_suite = github_with("[]", runs(&["pending"])).await;
    let empty_suite = github_with(EMPTY_ACTIONS_SUITE, runs(&["completed", "pending"])).await;
    let completed = github_with(EMPTY_ACTIONS_SUITE, runs(&["completed"])).await;

    let before_suite = read_checks(&no_suite).await.expect("read");
    let with_suite = read_checks(&empty_suite).await.expect("read");
    let done = read_checks(&completed).await.expect("read");

    assert_eq!(before_suite.held_apps, ["github-actions"]);
    assert_eq!(with_suite.held_apps, ["github-actions"]);
    assert!(done.held_apps.is_empty());
    assert!(!done.actions_unread);
}

/// Runs read to the page limit, every page full, may hide one that waits:
/// they hold, though every run read completed.
#[tokio::test]
async fn runs_not_all_read_hold() {
    let full_page = vec!["completed"; 100];
    let server = github_with("[]", runs(&full_page)).await;

    let checks = read_checks(&server).await.expect("read");

    assert_eq!(checks.held_apps, ["github-actions"]);
}

/// A token that may not read Actions, refused or told the repository is
/// not there, cannot tell whether a run waits: an empty Actions suite is
/// then held, a head with none is not stalled, and the read says so.
#[tokio::test]
async fn a_refused_actions_read_holds_only_an_empty_actions_suite() {
    for refusal in [
        ResponseTemplate::new(403)
            .set_body_string(r#"{"message":"Resource not accessible by integration"}"#),
        ResponseTemplate::new(404).set_body_string(r#"{"message":"Not Found"}"#),
    ] {
        let empty = github_with(EMPTY_ACTIONS_SUITE, refusal.clone()).await;
        let none = github_with("[]", refusal).await;

        let held = read_checks(&empty).await.expect("read");
        let not_held = read_checks(&none).await.expect("read");

        assert_eq!(held.held_apps, ["github-actions"]);
        assert!(not_held.held_apps.is_empty());
        assert!(held.actions_unread && not_held.actions_unread);
    }
}

/// GitHub failing to answer for the runs fails the read as an outage, to
/// be tried again, rather than reading as nothing held.
#[tokio::test]
async fn a_failing_actions_read_is_an_outage() {
    let server = github_with("[]", ResponseTemplate::new(502)).await;

    let read = read_checks(&server).await;

    let Err(error) = read else {
        panic!("a failing read fails");
    };
    assert!(matches!(
        error,
        GithubPullRequestsErr::Github(GithubError::Upstream(_))
    ));
    assert!(error.retryable());
}
