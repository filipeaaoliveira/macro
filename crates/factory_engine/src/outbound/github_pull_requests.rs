//! [`PullRequests`] on GitHub, as the Run's owner: with the token of the
//! GitHub account they linked to Macro, which the authentication service
//! keeps, through the `github` crate's user-token client.

use authentication_service_client::AuthServiceClient;
use github::domain::models::{
    GithubError, GithubMergeOutcome, GithubMergeRejection, GithubOpenPullRequest,
    GithubOpenPullRequestOutcome, GithubPullRequestDetails,
};
use github::domain::ports::GithubOauth;
use github::outbound::github_oauth_client::GithubOauthImpl;
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::models::{
    Check, CodeRepository, Merging, OpenPullRequest, Opening, PullRequest, PullRequestState,
};
use crate::domain::ports::{PullRequests, Retryable};

/// Why GitHub could not be asked as the Run's owner.
#[derive(Debug, thiserror::Error)]
pub enum GithubPullRequestsErr {
    /// The owner linked no GitHub account, or its token no longer works.
    #[error("the run's owner has no working GitHub account linked to Macro")]
    NotLinked,
    /// The authentication service did not give the token.
    #[error("the authentication service did not give the github token: {0:?}")]
    Token(rootcause::Report),
    /// GitHub did not answer, or answered with something unusable.
    #[error("github failed: {0}")]
    Github(#[from] GithubError),
}

/// A missing link stays missing until a person links GitHub; anything else
/// is a service's or GitHub's failure, worth trying again.
impl Retryable for GithubPullRequestsErr {
    fn retryable(&self) -> bool {
        !matches!(self, Self::NotLinked)
    }
}

/// GitHub, as each Run's owner.
#[derive(Clone)]
pub struct GithubPullRequests {
    auth: AuthServiceClient,
    github: GithubOauthImpl,
}

impl GithubPullRequests {
    /// Pull requests on GitHub, with tokens from `auth`.
    pub fn new(auth: AuthServiceClient) -> Self {
        Self {
            auth,
            github: GithubOauthImpl::default(),
        }
    }

    /// The owner's GitHub token, read on every call so a relinked account
    /// shows at once.
    async fn token(
        &self,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<String, GithubPullRequestsErr> {
        self.auth
            .get_github_access_token(owner.as_ref())
            .await
            .map_err(|error| GithubPullRequestsErr::Token(rootcause::report!("{error:#}")))?
            .map(|token| token.access_token)
            .ok_or(GithubPullRequestsErr::NotLinked)
    }
}

/// The pull request `number` as `details` describe it.
fn pull_request(
    repository: &CodeRepository,
    number: u64,
    details: GithubPullRequestDetails,
) -> PullRequest {
    let state = if details.merged_at.is_some() {
        PullRequestState::Merged
    } else if details.state == "open" {
        PullRequestState::Open
    } else {
        PullRequestState::Closed
    };
    PullRequest {
        number,
        url: format!("{}/pull/{number}", repository.url()),
        state,
        head_sha: details.head.and_then(|head| head.sha),
        checks: details.checks.map(|checks| {
            checks
                .into_iter()
                .map(|check| Check {
                    name: check.name,
                    status: check.status,
                    conclusion: check.conclusion,
                    url: check.url,
                })
                .collect()
        }),
    }
}

impl PullRequests for GithubPullRequests {
    type Err = GithubPullRequestsErr;

    #[tracing::instrument(err, skip(self, owner))]
    async fn default_branch(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
    ) -> Result<Option<String>, GithubPullRequestsErr> {
        // An owner with no linked account reaches no repository.
        let token = match self.token(owner).await {
            Ok(token) => token,
            Err(GithubPullRequestsErr::NotLinked) => return Ok(None),
            Err(error) => return Err(error),
        };
        let found = self
            .github
            .get_repository(&token, repository.owner(), repository.name())
            .await?;
        Ok(found.and_then(|found| found.default_branch))
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn find(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
    ) -> Result<Option<PullRequest>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let Some(number) = self
            .github
            .find_pull_request_by_head(&token, repository.owner(), repository.name(), branch)
            .await?
        else {
            return Ok(None);
        };
        let details = self
            .github
            .get_pull_request_details(&token, repository.owner(), repository.name(), number)
            .await?;
        Ok(Some(pull_request(repository, number, details)))
    }

    #[tracing::instrument(err, skip(self, owner, pull_request), fields(branch = %pull_request.branch))]
    async fn open(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        pull_request: &OpenPullRequest,
    ) -> Result<Opening, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let outcome = self
            .github
            .open_pull_request(
                &token,
                repository.owner(),
                repository.name(),
                &GithubOpenPullRequest {
                    head: pull_request.branch.clone(),
                    base: pull_request.base.clone(),
                    title: pull_request.title.clone(),
                    body: pull_request.body.clone(),
                },
            )
            .await?;
        Ok(match outcome {
            GithubOpenPullRequestOutcome::Opened(_) => Opening::Opened,
            GithubOpenPullRequestOutcome::Rejected(why) => Opening::Refused(why),
        })
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn merge(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        number: u64,
        head_sha: &str,
    ) -> Result<Merging, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        // The method a person merging in Macro gets when they pick none.
        let method = self
            .github
            .get_repository_merge_settings(&token, repository.owner(), repository.name())
            .await?
            .default_method();
        let Some(method) = method else {
            return Ok(Merging::Refused(
                "the repository allows no merge method".to_string(),
            ));
        };
        let outcome = self
            .github
            .merge_pull_request(
                &token,
                repository.owner(),
                repository.name(),
                number,
                method,
                Some(head_sha.to_string()),
            )
            .await?;
        Ok(match outcome {
            GithubMergeOutcome::Merged(_) => Merging::Merged,
            GithubMergeOutcome::Rejected {
                rejection: GithubMergeRejection::HeadChanged,
                ..
            } => Merging::HeadChanged,
            GithubMergeOutcome::Rejected { message, .. } => Merging::Refused(message),
        })
    }
}
