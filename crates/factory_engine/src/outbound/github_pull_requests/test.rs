use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use github::domain::models::GithubError;
use macro_user_id::user_id::MacroUserIdStr;
use upstream_error::{UpstreamError, UpstreamFailure};

use super::{GithubPullRequests, GithubPullRequestsErr, OwnerTokens};
use crate::domain::models::CodeRepository;
use crate::domain::ports::{PullRequests, Retryable};

/// An owner who linked no GitHub account, counting the reads of their token.
#[derive(Clone, Default)]
struct NoLink {
    reads: Arc<AtomicUsize>,
}

impl OwnerTokens for NoLink {
    async fn token(
        &self,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<Option<String>, rootcause::Report> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

fn repository() -> CodeRepository {
    CodeRepository::parse("https://github.com/macro/app").expect("valid")
}

/// An owner with no link reaches no repository, and every other call fails
/// for good without asking GitHub; the token is read again on each call, so
/// linking shows at once.
#[tokio::test]
async fn an_owner_with_no_link_fails_for_good_and_the_token_is_read_on_each_call() {
    let tokens = NoLink::default();
    let github = GithubPullRequests::new(tokens.clone());

    let access = github
        .repository_access(&julia(), &repository())
        .await
        .expect("no link reaches nothing");
    let found = github.find(&julia(), &repository(), "factory/run").await;
    let checked = github
        .find_with_checks(&julia(), &repository(), "factory/run")
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
