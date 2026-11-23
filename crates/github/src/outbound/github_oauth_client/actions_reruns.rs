//! Running a failed GitHub Actions job again, as the user.
//!
//! GitHub declines a rerun it cannot make, as for a job too old to rerun
//! or a token that may not write Actions: a 403 that is not throttling,
//! a 409 or a 422 is that answer, with GitHub's message. 401 and 404 are
//! refusals; throttling, 5xx and transport failures are
//! [`GithubError::Upstream`].

use reqwest::StatusCode;

use super::pull_requests::{is_repository_part, refusal};
use super::{
    GithubOauthImpl, REQUEST_TIMEOUT, USER_AGENT, response_text, throttling, transport_error,
    unexpected,
};
use crate::domain::models::GithubError;

#[cfg(test)]
mod test;

/// What GitHub answered a rerun of a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubJobRerun {
    /// GitHub queued the job to run again, as a new run of it.
    Started,
    /// GitHub would not rerun it, with its message.
    Declined(String),
}

#[derive(serde::Deserialize)]
struct ErrorBody {
    message: String,
}

impl GithubOauthImpl {
    /// Ask GitHub to run the GitHub Actions job `job_id` again, alone:
    /// its rerun is a new job of the same name.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn rerun_actions_job(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        job_id: u64,
    ) -> Result<GithubJobRerun, GithubError> {
        if !is_repository_part(owner) || !is_repository_part(repo) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a repository: {owner:?}/{repo:?}"
            )));
        }
        let response = self
            .client
            .post(format!(
                "{}/repos/{owner}/{repo}/actions/jobs/{job_id}/rerun",
                self.api_base_url()
            ))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if status.is_success() {
            return Ok(GithubJobRerun::Started);
        }
        let headers = response.headers().clone();
        let body = response_text(response).await;
        // A throttled request is a 403 too; it must not read as declined.
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(status, &headers, body, "failed to rerun a job"));
        }
        let declined = [
            StatusCode::FORBIDDEN,
            StatusCode::CONFLICT,
            StatusCode::UNPROCESSABLE_ENTITY,
        ];
        if declined.contains(&status) {
            let message = serde_json::from_str::<ErrorBody>(&body)
                .map(|error| error.message)
                .unwrap_or_else(|_| "GitHub declined to rerun the job.".to_string());
            return Ok(GithubJobRerun::Declined(message));
        }
        Err(refusal(status)
            .unwrap_or_else(|| unexpected(status, &headers, body, "failed to rerun a job")))
    }
}
