//! The `deploy` Step: once the Run merged, it waits for the signal its
//! repository declares in its verify file ([`VerifyFile`]) that the merge
//! is deployed, so the `verify` Step checks the change where it runs.
//!
//! It ends `deployed` once GitHub's newest deployment of the merge commit
//! to the declared environment succeeded (or succeeded and was replaced
//! since by a later one), `deploy_failed` when it failed, and `timeout`
//! when none succeeded within [`DEPLOY_WAIT`]. A repository that declares
//! no deploy needs no wait: `not_needed`. One that turned the verify Phase
//! off ends `off`, and one whose file does not read `unreadable`.

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

/// How long `deploy` waits between two reads of the deployment.
const DEPLOY_POLL: Duration = Duration::from_secs(20);

impl<G, R, M> PullRequestSteps<G, R, M>
where
    G: PullRequests,
    R: RunRepository + MainHoldRepository,
    M: MainWatches,
{
    /// The verify Phase the Run's base branch declares, which the agent's
    /// branch changes only as a Control File: the default when it has no
    /// verify file.
    pub(crate) async fn verify_file(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Result<VerifyFile, VerifyFileError>, PullRequestStepError> {
        let (repository, recorded) = Self::repository(input)?;
        let text = self
            .github
            .file_text(input.owner, &repository, &recorded.base_branch, VERIFY_FILE)
            .await
            .map_err(PullRequestStepError::github)?;
        Ok(VerifyFile::read(text.as_deref()))
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

    /// The newest deployment of `commit` to `environment`, as the Run's
    /// owner reads it.
    pub(crate) async fn deployment(
        &self,
        input: &StepInput<'_>,
        commit: &str,
        environment: &str,
    ) -> Result<Option<Deployment>, PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;
        self.github
            .deployment(input.owner, &repository, commit, environment)
            .await
            .map_err(PullRequestStepError::github)
    }

    /// Wait for the Run's merge to be deployed, as its repository declares.
    pub async fn deploy(&self, input: StepInput<'_>) -> Result<StepReport, PullRequestStepError> {
        let file = match self.verify_file(&input).await? {
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
                format!("The change is merged. The repository turns its verify phase off in {VERIFY_FILE}."),
            ));
        }
        let (pull_request, commit) = self.merged_commit(&input).await?;
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
            let deployment = self.deployment(&input, &commit, &environment).await?;
            if let Some(deployment) = deployment {
                let at = deployment
                    .url
                    .as_deref()
                    .map(|url| format!(" at {url}"))
                    .unwrap_or_default();
                match deployment.state {
                    DeploymentState::Succeeded => {
                        return Ok(report(
                            "deployed",
                            format!(
                                "{}\n\nThe merged commit {commit} is deployed to {environment}{at}.",
                                named(&pull_request)
                            ),
                        ));
                    }
                    DeploymentState::Replaced => {
                        return Ok(report(
                            "deployed",
                            format!(
                                "{}\n\nThe merged commit {commit} was deployed to {environment}{at}, and a later deployment has replaced it since.",
                                named(&pull_request)
                            ),
                        ));
                    }
                    DeploymentState::Failed => {
                        return Ok(report(
                            "deploy_failed",
                            format!(
                                "{}\n\nThe deployment of the merged commit {commit} to {environment} failed.",
                                named(&pull_request)
                            ),
                        ));
                    }
                    DeploymentState::Waiting => {}
                }
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
