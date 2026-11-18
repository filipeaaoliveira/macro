//! Reads of a repository's deployments as the user: the newest deployments
//! of a commit, or of any commit, to an environment, and how each went.

use super::GithubOauthImpl;
use crate::domain::models::GithubError;

#[cfg(test)]
mod test;

/// The most statuses of a deployment read, newest first, to find the
/// newest that names where the deployment is reached.
const STATUSES_READ: &str = "30";

#[derive(serde::Deserialize)]
struct DeploymentItem {
    id: u64,
    sha: String,
}

#[derive(serde::Deserialize)]
struct DeploymentStatusItem {
    state: String,
    #[serde(default)]
    environment_url: Option<String>,
}

/// A deployment of a commit to an environment, with how its newest status
/// says it went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubDeployment {
    /// Its id in its repository.
    pub id: u64,
    /// The commit it deploys.
    pub sha: String,
    /// The state of its newest status, such as `success`, `failure` or
    /// `in_progress`; none before it has a status.
    pub state: Option<String>,
    /// Where it is reached, as the newest of its statuses that says so
    /// names it; a later status, such as `inactive`, often names none.
    pub environment_url: Option<String>,
}

impl GithubOauthImpl {
    /// The newest `count` deployments to `environment`, newest first: of
    /// the commit `sha` only, when given. Each with its newest status. A
    /// repository the user cannot reach is
    /// [`GithubError::RepositoryUnavailable`].
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_deployments(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: Option<&str>,
        environment: &str,
        count: u8,
    ) -> Result<Vec<GithubDeployment>, GithubError> {
        let per_page = count.to_string();
        let mut query = vec![("environment", environment), ("per_page", per_page.as_str())];
        if let Some(sha) = sha {
            query.push(("sha", sha));
        }
        // GitHub lists deployments newest first, and a deployment's
        // statuses newest first too.
        let deployments: Vec<DeploymentItem> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/deployments"),
                &query,
                "failed to list deployments",
            )
            .await?;

        let mut read = Vec::with_capacity(deployments.len());
        for deployment in deployments {
            let statuses: Vec<DeploymentStatusItem> = self
                .read(
                    access_token,
                    &format!(
                        "/repos/{owner}/{repo}/deployments/{}/statuses",
                        deployment.id
                    ),
                    &[("per_page", STATUSES_READ)],
                    "failed to list deployment statuses",
                )
                .await?;
            let state = statuses.first().map(|status| status.state.clone());
            let environment_url = statuses
                .into_iter()
                .filter_map(|status| status.environment_url)
                .find(|url| !url.is_empty());
            read.push(GithubDeployment {
                id: deployment.id,
                sha: deployment.sha,
                state,
                environment_url,
            });
        }
        Ok(read)
    }
}
