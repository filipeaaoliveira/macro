use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use github::domain::models::GithubError;
use github::outbound::github_oauth_client::GithubOauthImpl;
use github::test_support::{MockResponse, MockServer};
use macro_user_id::user_id::MacroUserIdStr;
use upstream_error::{UpstreamError, UpstreamFailure};

use super::{GithubPullRequests, GithubPullRequestsErr, OwnerTokens};
use crate::domain::models::CodeRepository;
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

/// GitHub answering one CI read of a head: the pull request, its suites
/// (`suites`, as GitHub's JSON list), no statuses, and its workflow runs
/// (`runs`, a status line and body).
fn ci_read(suites: &str, runs: (&'static str, String)) -> MockServer {
    MockServer::start_many(vec![
        MockResponse {
            status: "200 OK",
            body: r#"[{"number":7,"html_url":"https://github.com/macro/app/pull/7","state":"open","merged_at":null,"head":{"sha":"abc123"}}]"#.to_string(),
        },
        MockResponse {
            status: "200 OK",
            body: format!(r#"{{"total_count":1,"check_suites":{suites}}}"#),
        },
        MockResponse {
            status: "200 OK",
            body: r#"{"state":"pending","statuses":[]}"#.to_string(),
        },
        MockResponse {
            status: runs.0,
            body: runs.1,
        },
    ])
}

const EMPTY_ACTIONS_SUITE: &str = r#"[{"id":1,"status":"queued","conclusion":null,"latest_check_runs_count":0,"app":{"slug":"github-actions"}}]"#;

/// The held apps one CI read of `server` finds.
async fn held_apps(server: &MockServer) -> Vec<String> {
    let github = GithubPullRequests::with_client(
        Tokens {
            linked: Some("token"),
            ..Tokens::default()
        },
        GithubOauthImpl::with_api_base_url(server.base_url.clone()),
    );
    let (_, checks) = github
        .find_with_checks(&julia(), &repository(), "factory/run", false)
        .await
        .expect("read")
        .expect("a pull request");
    checks.held_apps
}

/// An Actions run still pending holds CI as GitHub Actions'; runs that all
/// completed hold nothing.
#[tokio::test]
async fn a_pending_actions_run_holds_and_completed_ones_do_not() {
    let pending = ci_read(
        EMPTY_ACTIONS_SUITE,
        (
            "200 OK",
            r#"{"total_count":2,"workflow_runs":[{"status":"completed"},{"status":"pending"}]}"#
                .to_string(),
        ),
    );
    let held = held_apps(&pending).await;
    let requests = pending.finish_all();
    let completed = ci_read(
        EMPTY_ACTIONS_SUITE,
        (
            "200 OK",
            r#"{"total_count":1,"workflow_runs":[{"status":"completed"}]}"#.to_string(),
        ),
    );
    let not_held = held_apps(&completed).await;
    completed.finish_all();

    assert_eq!(held, ["github-actions"]);
    assert!(not_held.is_empty());
    assert!(requests[3].starts_with("GET /repos/macro/app/actions/runs?head_sha=abc123&"));
}

/// A token that may not read Actions cannot tell whether a run waits: an
/// empty Actions suite is then held, and a head with none is not stalled.
#[tokio::test]
async fn a_refused_actions_read_holds_only_an_empty_actions_suite() {
    let refused = || {
        (
            "403 Forbidden",
            r#"{"message":"Resource not accessible by integration"}"#.to_string(),
        )
    };
    let empty = ci_read(EMPTY_ACTIONS_SUITE, refused());
    let held = held_apps(&empty).await;
    empty.finish_all();
    let none = ci_read("[]", refused());
    let not_held = held_apps(&none).await;
    none.finish_all();

    assert_eq!(held, ["github-actions"]);
    assert!(not_held.is_empty());
}
