//! Reads of a repository's deployments as the user: how the newest
//! deployment of a commit to an environment went.

use super::GithubOauthImpl;
use crate::domain::models::GithubError;

#[cfg(test)]
mod test;

#[derive(serde::Deserialize)]
struct DeploymentItem {
    id: u64,
}

#[derive(serde::Deserialize)]
struct DeploymentStatusItem {
    state: String,
    #[serde(default)]
    environment_url: Option<String>,
}

/// The newest deployment of a commit to an environment, and its newest
/// status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubDeployment {
    /// Its id in its repository.
    pub id: u64,
    /// The state of its newest status, such as `success`, `failure` or
    /// `in_progress`; none before it has a status.
    pub state: Option<String>,
    /// Where the newest status says the deployment is reached, if it does.
    pub environment_url: Option<String>,
}

impl GithubOauthImpl {
    /// The newest deployment of the commit `sha` to `environment`, with its
    /// newest status; `None` when there is none yet. A repository the user
    /// cannot reach is [`GithubError::RepositoryUnavailable`].
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_latest_deployment(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
        environment: &str,
    ) -> Result<Option<GithubDeployment>, GithubError> {
        // GitHub lists deployments newest first.
        let deployments: Vec<DeploymentItem> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/deployments"),
                &[
                    ("sha", sha),
                    ("environment", environment),
                    ("per_page", "1"),
                ],
                "failed to list deployments",
            )
            .await?;
        let Some(deployment) = deployments.into_iter().next() else {
            return Ok(None);
        };

        // And a deployment's statuses newest first too.
        let statuses: Vec<DeploymentStatusItem> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/deployments/{}/statuses", deployment.id),
                &[("per_page", "1")],
                "failed to list deployment statuses",
            )
            .await?;
        let status = statuses.into_iter().next();
        Ok(Some(GithubDeployment {
            id: deployment.id,
            state: status.as_ref().map(|status| status.state.clone()),
            environment_url: status
                .and_then(|status| status.environment_url)
                .filter(|url| !url.is_empty()),
        }))
    }
}
