use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use github::domain::models::GithubError;
use github::outbound::github_oauth_client::GithubOauthImpl;
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
