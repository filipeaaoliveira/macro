//! [`PullRequests`] on GitHub, as the Run's owner: with the token of the
//! GitHub account they linked to Macro, which the authentication service
//! keeps, through the `github` crate's user-token client.

#[cfg(test)]
mod test;

use authentication_service_client::AuthServiceClient;
use github::domain::models::{
    GithubBranchDeletion, GithubError, GithubMergeOutcome, GithubMergeRejection,
    GithubOpenPullRequest, GithubOpenPullRequestOutcome, GithubPullRequestHead,
};
use github::domain::ports::GithubOauth;
use github::outbound::github_oauth_client::GithubOauthImpl;
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::models::{
    BranchDeletion, Check, CheckState, CheckSuite, CodeRepository, CommitStatus, Deployment,
    DeploymentState, HeadChecks, Merging, OpenPullRequest, Opening, PullRequest, PullRequestState,
    RepositoryAccess, RequiredCheck, RequiredChecks, is_run_branch,
};
use crate::domain::ports::{PullRequests, Retryable};
use crate::domain::review_rounds::ChangedFile;

/// GitHub Actions' app, whose workflow runs say whether a run still waits.
const ACTIONS_APP: &str = "github-actions";

/// Why GitHub could not be asked as the Run's owner.
#[derive(Debug, thiserror::Error)]
pub enum GithubPullRequestsErr {
    /// The owner linked no GitHub account, or its token no longer works.
    #[error("the run's owner has no working GitHub account linked to Macro")]
    NotLinked,
    /// The token could not be read.
    #[error("the github token could not be read: {0:?}")]
    Token(rootcause::Report),
    /// GitHub did not answer, or refused.
    #[error("github failed: {0}")]
    Github(#[from] GithubError),
    /// A branch to delete that is no Run's branch: the factory deletes only
    /// the branches it made.
    #[error("not a run's branch: {0:?}")]
    NotRunBranch(String),
}

/// An outage passes: GitHub throttling or failing, a request that got no
/// answer, or a token that could not be read. A refusal does not: a token
/// GitHub no longer takes (401), a refusal that is not throttling (403), a
/// repository the owner cannot reach (404), a missing link, or an answer the
/// client did not expect.
impl Retryable for GithubPullRequestsErr {
    fn retryable(&self) -> bool {
        match self {
            Self::Token(_) | Self::Github(GithubError::Upstream(_)) => true,
            Self::NotLinked | Self::Github(_) | Self::NotRunBranch(_) => false,
        }
    }
}

/// Where the Run's owner's GitHub token comes from.
pub trait OwnerTokens: Send + Sync + 'static {
    /// The token of the GitHub account `owner` linked, if they linked one
    /// that still works.
    fn token(
        &self,
        owner: &MacroUserIdStr<'static>,
    ) -> impl Future<Output = Result<Option<String>, rootcause::Report>> + Send;
}

/// The authentication service keeps each person's GitHub link.
impl OwnerTokens for AuthServiceClient {
    async fn token(
        &self,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<Option<String>, rootcause::Report> {
        self.get_github_access_token(owner.as_ref())
            .await
            .map(|token| token.map(|token| token.access_token))
            .map_err(|error| rootcause::report!("{error:#}"))
    }
}

/// GitHub, as each Run's owner.
#[derive(Clone)]
pub struct GithubPullRequests<T> {
    tokens: T,
    github: GithubOauthImpl,
}

impl<T: OwnerTokens> GithubPullRequests<T> {
    /// Pull requests on GitHub, with tokens from `tokens`.
    pub fn new(tokens: T) -> Self {
        Self {
            tokens,
            github: GithubOauthImpl::default(),
        }
    }

    /// Pull requests on the GitHub `github` reaches, with tokens from
    /// `tokens`: a stand-in's, in tests.
    #[cfg(test)]
    pub fn with_client(tokens: T, github: GithubOauthImpl) -> Self {
        Self { tokens, github }
    }

    /// The owner's GitHub token, read on every call so a relinked account
    /// shows at once.
    async fn token(
        &self,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<String, GithubPullRequestsErr> {
        self.tokens
            .token(owner)
            .await
            .map_err(GithubPullRequestsErr::Token)?
            .ok_or(GithubPullRequestsErr::NotLinked)
    }

    /// The pull request from `branch`, with `token`.
    async fn head(
        &self,
        token: &str,
        repository: &CodeRepository,
        branch: &str,
    ) -> Result<Option<PullRequest>, GithubPullRequestsErr> {
        let found = self
            .github
            .find_pull_request_by_head(token, repository.owner(), repository.name(), branch)
            .await?;
        Ok(found.map(pull_request))
    }

    /// The checks on the commit `sha` of `repository`, with `token`: all
    /// its check runs when `all_runs`, else only those of a failed suite.
    async fn checks_on(
        &self,
        token: &str,
        repository: &CodeRepository,
        sha: &str,
        all_runs: bool,
    ) -> Result<HeadChecks, GithubPullRequestsErr> {
        let (owner_name, name) = (repository.owner(), repository.name());
        let suites = self
            .github
            .list_check_suites(token, owner_name, name, sha)
            .await?;
        let statuses = self
            .github
            .list_commit_statuses(token, owner_name, name, sha)
            .await?;
        let mut truncated = !suites.complete || !statuses.complete;
        let suites: Vec<CheckSuite> = suites
            .items
            .into_iter()
            .map(|suite| CheckSuite {
                id: suite.id,
                app: suite.app,
                state: CheckState::of_check(&suite.status, suite.conclusion.as_deref()),
                check_runs: suite.check_runs,
                created_at: suite.created_at,
            })
            .collect();
        let statuses = statuses
            .items
            .into_iter()
            .map(|status| CommitStatus {
                context: status.context,
                state: CheckState::of_status(&status.state),
                url: status.target_url,
            })
            .collect();

        // Runs are read to find required checks by name, or to name what
        // failed in a failed suite; only the failed ones are kept then.
        let suite_failed = suites
            .iter()
            .any(|suite| matches!(suite.state, CheckState::Failed(_)));
        let runs = if all_runs || suite_failed {
            let runs = self
                .github
                .list_check_runs(token, owner_name, name, sha)
                .await?;
            truncated |= !runs.complete;
            runs.items
                .into_iter()
                .map(|run| Check {
                    name: run.name,
                    state: CheckState::of_check(&run.status, run.conclusion.as_deref()),
                    url: run.html_url,
                    app_id: run.app_id,
                })
                .filter(|run| all_runs || matches!(run.state, CheckState::Failed(_)))
                .collect()
        } else {
            Vec::new()
        };
        // A GitHub Actions run not finished, as one a concurrency group
        // holds or one whose suite GitHub has not made yet, shows only in the
        // head's workflow runs; runs not all read may hide one, so they hold
        // too. A token that may not read Actions cannot tell: it holds only
        // while an Actions suite sits empty, so a repository the App may not
        // read Actions in is not stalled.
        let mut held_apps = Vec::new();
        let mut actions_unread = false;
        match self
            .github
            .list_workflow_runs(token, owner_name, name, sha)
            .await
        {
            Ok(runs) => {
                let waiting = runs.items.iter().any(|run| run.status.is_waiting());
                if waiting || !runs.complete {
                    held_apps.push(ACTIONS_APP.to_string());
                }
            }
            Err(error @ (GithubError::Forbidden | GithubError::RepositoryUnavailable)) => {
                tracing::warn!(
                    %error,
                    "the github token may not read actions runs; an empty actions suite is held"
                );
                actions_unread = true;
                let actions_empty = suites
                    .iter()
                    .any(|suite| suite.app == ACTIONS_APP && suite.check_runs == 0);
                if actions_empty {
                    held_apps.push(ACTIONS_APP.to_string());
                }
            }
            Err(error) => return Err(error.into()),
        }
        Ok(HeadChecks {
            suites,
            statuses,
            runs,
            truncated,
            held_apps,
            actions_unread,
        })
    }
}

/// The pull request `found` describes.
fn pull_request(found: GithubPullRequestHead) -> PullRequest {
    let state = if found.merged {
        PullRequestState::Merged
    } else if found.state == "open" {
        PullRequestState::Open
    } else {
        PullRequestState::Closed
    };
    PullRequest {
        number: found.number,
        url: found.html_url,
        state,
        head_sha: found.head_sha,
        merge_commit: found.merge_commit_sha,
    }
}

impl<T: OwnerTokens> PullRequests for GithubPullRequests<T> {
    type Err = GithubPullRequestsErr;

    #[tracing::instrument(err, skip(self, owner))]
    async fn repository_access(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
    ) -> Result<Option<RepositoryAccess>, GithubPullRequestsErr> {
        // An owner with no linked account reaches no repository.
        let token = match self.token(owner).await {
            Ok(token) => token,
            Err(GithubPullRequestsErr::NotLinked) => return Ok(None),
            Err(error) => return Err(error),
        };
        let access = self
            .github
            .get_repository_access(&token, repository.owner(), repository.name())
            .await?;
        Ok(access.and_then(|access| {
            Some(RepositoryAccess {
                default_branch: access.default_branch?,
                can_push: access.push,
                deletes_merged_branches: access.delete_branch_on_merge,
            })
        }))
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn file_text(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
        path: &str,
    ) -> Result<Option<String>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        Ok(self
            .github
            .get_file_text(&token, repository.owner(), repository.name(), branch, path)
            .await?)
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn changed_paths(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        base: &str,
        branch: &str,
    ) -> Result<Option<Vec<String>>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        Ok(self
            .github
            .list_changed_paths(&token, repository.owner(), repository.name(), base, branch)
            .await?)
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn branch_head(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
    ) -> Result<Option<String>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        Ok(self
            .github
            .get_branch_head(&token, repository.owner(), repository.name(), branch)
            .await?)
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn changed_files(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        base: &str,
        head: &str,
    ) -> Result<Option<Vec<ChangedFile>>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let files = self
            .github
            .compare_files(&token, repository.owner(), repository.name(), base, head)
            .await?;
        Ok(files.map(|files| {
            files
                .into_iter()
                .map(|file| {
                    ChangedFile::from_patch(file.path, file.previous_path, file.patch.as_deref())
                })
                .collect()
        }))
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn find(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
    ) -> Result<Option<PullRequest>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        self.head(&token, repository, branch).await
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn find_with_checks(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
        all_runs: bool,
    ) -> Result<Option<(PullRequest, HeadChecks)>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let Some(pull_request) = self.head(&token, repository, branch).await? else {
            return Ok(None);
        };
        let checks = self
            .checks_on(&token, repository, &pull_request.head_sha, all_runs)
            .await?;
        Ok(Some((pull_request, checks)))
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn branch_checks(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
        all_runs: bool,
    ) -> Result<Option<(String, HeadChecks)>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let Some(head) = self
            .github
            .get_branch_head(&token, repository.owner(), repository.name(), branch)
            .await?
        else {
            return Ok(None);
        };
        let checks = self.checks_on(&token, repository, &head, all_runs).await?;
        Ok(Some((head, checks)))
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn required_checks(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        base: &str,
    ) -> Result<Option<RequiredChecks>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let required = self
            .github
            .get_required_checks(&token, repository.owner(), repository.name(), base)
            .await?;
        Ok(required.map(|required| RequiredChecks {
            checks: required
                .checks
                .into_iter()
                .map(|check| RequiredCheck {
                    name: check.context,
                    app_id: check.app_id,
                })
                .collect(),
            complete: required.complete,
        }))
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn deployments(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        sha: Option<&str>,
        environment: &str,
        count: u8,
    ) -> Result<Vec<Deployment>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let deployments = self
            .github
            .list_deployments(
                &token,
                repository.owner(),
                repository.name(),
                sha,
                environment,
                count,
            )
            .await?;
        Ok(deployments
            .into_iter()
            .map(|deployment| Deployment {
                commit: deployment.sha,
                state: DeploymentState::of_status(deployment.state.as_deref()),
                url: deployment.environment_url,
            })
            .collect())
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn first_parent(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        sha: &str,
    ) -> Result<Option<String>, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        let parents = self
            .github
            .get_commit_parents(&token, repository.owner(), repository.name(), sha)
            .await?;
        Ok(parents.into_iter().next())
    }

    #[tracing::instrument(err, skip(self, owner))]
    async fn contains(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        ancestor: &str,
        head: &str,
    ) -> Result<bool, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        Ok(self
            .github
            .commit_contains(
                &token,
                repository.owner(),
                repository.name(),
                ancestor,
                head,
            )
            .await?)
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

    /// The Step that asks logs a failure, so it is not logged here too.
    #[tracing::instrument(skip(self, owner))]
    async fn has_open_pull_request_onto(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
    ) -> Result<bool, GithubPullRequestsErr> {
        let token = self.token(owner).await?;
        Ok(self
            .github
            .has_open_pull_request_onto(&token, repository.owner(), repository.name(), branch)
            .await?)
    }

    /// Only a Run's branch is deleted: any other is refused before GitHub
    /// is asked. The Step that asks logs a failure, so it is not logged
    /// here too.
    #[tracing::instrument(skip(self, owner))]
    async fn delete_branch(
        &self,
        owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        branch: &str,
    ) -> Result<BranchDeletion, GithubPullRequestsErr> {
        if !is_run_branch(branch) {
            return Err(GithubPullRequestsErr::NotRunBranch(branch.to_string()));
        }
        let token = self.token(owner).await?;
        let deletion = self
            .github
            .delete_branch(&token, repository.owner(), repository.name(), branch)
            .await?;
        Ok(match deletion {
            GithubBranchDeletion::Deleted => BranchDeletion::Deleted,
            GithubBranchDeletion::Missing => BranchDeletion::Missing,
        })
    }
}
