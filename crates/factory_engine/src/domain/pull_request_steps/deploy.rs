//! The `deploy` Step: once the Run merged, it waits for the signal its
//! repository declares in its verify file ([`VerifyFile`]) that the merge
//! is deployed, so the `verify` Step checks the change where it runs.
//!
//! The verify file is read at the merge commit's first parent: the base as
//! it was before the Run's change, never after it, so a Run cannot turn its
//! own verification off by changing the file.
//!
//! It ends `deployed` once the merge is deployed to the declared
//! environment ([`DeployState`]), `deploy_failed` when its deployment
//! failed and no later one deployed it, and `timeout` when neither
//! happened within [`DEPLOY_WAIT`]. A repository that declares no deploy
//! needs no wait: `not_needed`. One that turned the verify Phase off ends
//! `off`, and one whose file does not read `unreadable`.

#[cfg(test)]
mod test;

use std::time::Duration;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::ports::RunRepository;

use super::{PullRequestStepError, PullRequestSteps, named, report};
use crate::domain::models::{
    Deployment, DeploymentState, PullRequest, PullRequestState, StepInput, StepReport,
};
use crate::domain::ports::{MainWatches, PullRequests};
use crate::domain::verification::{VERIFY_FILE, VerifyFile, VerifyFileError};

/// How long `deploy` waits for the merge to be deployed: short of a Step
/// attempt's 30 minutes, as a CI wait is, so the Step ends `timeout`
/// rather than failing.
pub const DEPLOY_WAIT: Duration = Duration::from_secs(25 * 60);

/// How long `deploy` waits between two reads of the deployments.
const DEPLOY_POLL: Duration = Duration::from_secs(20);

/// How many of the environment's newest deployments are read for one of a
/// later commit that deployed the merge with it.
const LATER_DEPLOYMENTS: u8 = 5;

/// The longest deployment address the acceptance checks are told: a longer
/// one is not passed on, so a check's command stays within what a round
/// keeps.
pub(crate) const MAX_URL_BYTES: usize = 512;

/// Whether a merge commit is deployed to an environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DeployState {
    /// It is: by its own deployment, or by that of `later`, a later commit
    /// that contains it. `url` is where, when the deployment says and the
    /// address is not too long to pass on.
    Deployed {
        /// Where the deployment is reached.
        url: Option<String>,
        /// The later commit whose deployment deployed it, if not its own.
        later: Option<String>,
    },
    /// Its own deployment failed, and no later one deployed it.
    Failed,
    /// Nothing deployed it yet.
    Waiting,
}

impl<G, R, M> PullRequestSteps<G, R, M>
where
    G: PullRequests,
    R: RunRepository + MainHoldRepository,
    M: MainWatches,
{
    /// The verify Phase the Run's repository declares at `at`, a branch or
    /// a commit: the default when it has no verify file.
    pub(crate) async fn verify_file(
        &self,
        input: &StepInput<'_>,
        at: &str,
    ) -> Result<Result<VerifyFile, VerifyFileError>, PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;
        let text = self
            .github
            .file_text(input.owner, &repository, at, VERIFY_FILE)
            .await
            .map_err(PullRequestStepError::github)?;
        Ok(VerifyFile::read(text.as_deref()))
    }

    /// The verify Phase the Run's base branch declares now, before the
    /// Run merged: the agent's branch changes it only as a Control File.
    pub(crate) async fn base_verify_file(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Result<VerifyFile, VerifyFileError>, PullRequestStepError> {
        let (_, recorded) = Self::repository(input)?;
        self.verify_file(input, &recorded.base_branch).await
    }

    /// The verify Phase the Run's base declared just before its merge
    /// commit `commit`: at the commit's first parent, so not changed by the
    /// Run's own change.
    pub(crate) async fn verify_file_before(
        &self,
        input: &StepInput<'_>,
        commit: &str,
    ) -> Result<Result<VerifyFile, VerifyFileError>, PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;
        let before = self
            .github
            .first_parent(input.owner, &repository, commit)
            .await
            .map_err(PullRequestStepError::github)?
            .ok_or(PullRequestStepError::NoMergeParent)?;
        self.verify_file(input, &before).await
    }

    /// The Run's merged pull request, and the commit its merge made on the
    /// base.
    pub(crate) async fn merged_commit(
        &self,
        input: &StepInput<'_>,
    ) -> Result<(PullRequest, String), PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;
        let pull_request = self
            .find(input, &repository)
            .await?
            .ok_or(PullRequestStepError::NoPullRequest)?;
        if pull_request.state != PullRequestState::Merged {
            return Err(PullRequestStepError::NotMerged);
        }
        let commit = pull_request
            .merge_commit
            .clone()
            .ok_or(PullRequestStepError::NoMergeCommit)?;
        Ok((pull_request, commit))
    }

    /// Whether the merge commit `commit` is deployed to `environment`:
    /// once its own newest deployment succeeded (or succeeded and was
    /// replaced since), or a successful deployment of a later commit that
    /// contains it, among the environment's newest, deployed it with that
    /// commit. A deployment of its own that failed or was cancelled is a
    /// failure only when no later one deployed it.
    pub(crate) async fn deploy_state(
        &self,
        input: &StepInput<'_>,
        commit: &str,
        environment: &str,
    ) -> Result<DeployState, PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;
        let own = self
            .github
            .deployments(input.owner, &repository, Some(commit), environment, 1)
            .await
            .map_err(PullRequestStepError::github)?
            .into_iter()
            .next();
        if let Some(own) = &own
            && matches!(
                own.state,
                DeploymentState::Succeeded | DeploymentState::Replaced
            )
        {
            return Ok(DeployState::Deployed {
                url: passed_on(own),
                later: None,
            });
        }

        let newest = self
            .github
            .deployments(
                input.owner,
                &repository,
                None,
                environment,
                LATER_DEPLOYMENTS,
            )
            .await
            .map_err(PullRequestStepError::github)?;
        for later in newest.iter().filter(|later| {
            later.commit != commit
                && matches!(
                    later.state,
                    DeploymentState::Succeeded | DeploymentState::Replaced
                )
        }) {
            let contains = self
                .github
                .contains(input.owner, &repository, commit, &later.commit)
                .await
                .map_err(PullRequestStepError::github)?;
            if contains {
                return Ok(DeployState::Deployed {
                    url: passed_on(later),
                    later: Some(later.commit.clone()),
                });
            }
        }

        Ok(match own.map(|own| own.state) {
            Some(DeploymentState::Failed) => DeployState::Failed,
            _ => DeployState::Waiting,
        })
    }

    /// Wait for the Run's merge to be deployed, as its repository declared
    /// before the merge.
    pub async fn deploy(&self, input: StepInput<'_>) -> Result<StepReport, PullRequestStepError> {
        let (pull_request, commit) = self.merged_commit(&input).await?;
        let file = match self.verify_file_before(&input, &commit).await? {
            Ok(file) => file,
            Err(error) => {
                return Ok(report(
                    "unreadable",
                    format!("The change is merged, and it was not verified: {error}"),
                ));
            }
        };
        if !file.verify {
            return Ok(report(
                "off",
                format!(
                    "The change is merged. The repository turns its verify phase off in {VERIFY_FILE}."
                ),
            ));
        }
        let Some(deploy) = file.deploy else {
            return Ok(report(
                "not_needed",
                format!(
                    "{}\n\nThe repository declares no deploy in {VERIFY_FILE}: the merged commit {commit} is verified as it is.",
                    named(&pull_request)
                ),
            ));
        };

        let environment = deploy.environment;
        let deadline = tokio::time::Instant::now() + DEPLOY_WAIT;
        loop {
            match self.deploy_state(&input, &commit, &environment).await? {
                DeployState::Deployed { url, later } => {
                    let at = url.map(|url| format!(" at {url}")).unwrap_or_default();
                    let how = match later {
                        Some(later) => format!(", with the later commit {later} that contains it"),
                        None => String::new(),
                    };
                    return Ok(report(
                        "deployed",
                        format!(
                            "{}\n\nThe merged commit {commit} is deployed to {environment}{at}{how}.",
                            named(&pull_request)
                        ),
                    ));
                }
                DeployState::Failed => {
                    return Ok(report(
                        "deploy_failed",
                        format!(
                            "{}\n\nThe deployment of the merged commit {commit} to {environment} failed, and no later one deployed it.",
                            named(&pull_request)
                        ),
                    ));
                }
                DeployState::Waiting => {}
            }
            if tokio::time::Instant::now() + DEPLOY_POLL > deadline {
                return Ok(report(
                    "timeout",
                    format!(
                        "{}\n\nThe merged commit {commit} was not deployed to {environment} within {} minutes.",
                        named(&pull_request),
                        DEPLOY_WAIT.as_secs() / 60
                    ),
                ));
            }
            tokio::time::sleep(DEPLOY_POLL).await;
        }
    }
}

/// Where `deployment` is reached, when it says and the address is short
/// enough to pass on to the checks.
fn passed_on(deployment: &Deployment) -> Option<String> {
    deployment
        .url
        .clone()
        .filter(|url| url.len() <= MAX_URL_BYTES)
}
