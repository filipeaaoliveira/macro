//! Calls that take a branch to a merged pull request as the user: what they
//! may do with a repository, the pull request from a branch, opening one,
//! and the checks and statuses on its head. Each read is one request for
//! only what it needs, as they are read again and again while CI runs.
//!
//! A refusal GitHub answers is told apart from an outage: 401 is a token
//! that no longer works ([`GithubError::ReauthenticationRequired`]), 403
//! that is not throttling is [`GithubError::Forbidden`], and 404 is a
//! repository or commit the user cannot reach
//! ([`GithubError::RepositoryUnavailable`]). Throttling, 5xx and transport
//! failures are [`GithubError::Upstream`].

use reqwest::StatusCode;
use serde::de::DeserializeOwned;

use super::{
    GithubErrorBody, GithubOauthImpl, REQUEST_TIMEOUT, USER_AGENT, response_text, throttling,
    transport_error, unexpected,
};
use crate::domain::models::{
    GithubCheckSuite, GithubCommitStatus, GithubError, GithubOpenPullRequest,
    GithubOpenPullRequestOutcome, GithubPullRequestCheckRun, GithubPullRequestHead,
    GithubRepositoryAccess,
};

#[cfg(test)]
mod test;

/// The most items one page of a list holds.
const PAGE: &str = "100";

/// What a refused read is, once throttling is ruled out.
fn refusal(status: StatusCode) -> Option<GithubError> {
    match status {
        StatusCode::UNAUTHORIZED => Some(GithubError::ReauthenticationRequired),
        StatusCode::FORBIDDEN => Some(GithubError::Forbidden),
        StatusCode::NOT_FOUND => Some(GithubError::RepositoryUnavailable),
        _ => None,
    }
}

#[derive(serde::Deserialize)]
struct RepositoryResponse {
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    permissions: Option<RepositoryPermissions>,
}

#[derive(serde::Deserialize)]
struct RepositoryPermissions {
    #[serde(default)]
    push: bool,
}

#[derive(serde::Deserialize)]
struct PullRequestListItem {
    number: u64,
    html_url: String,
    state: String,
    merged_at: Option<String>,
    head: PullRequestHeadRef,
}

#[derive(serde::Deserialize)]
struct PullRequestHeadRef {
    sha: String,
}

#[derive(serde::Deserialize)]
struct CheckSuitesResponse {
    check_suites: Vec<CheckSuiteResponse>,
}

#[derive(serde::Deserialize)]
struct CheckSuiteResponse {
    id: u64,
    status: Option<String>,
    conclusion: Option<String>,
    #[serde(default)]
    latest_check_runs_count: u64,
    app: Option<AppResponse>,
}

#[derive(serde::Deserialize)]
struct AppResponse {
    slug: Option<String>,
}

#[derive(serde::Deserialize)]
struct CombinedStatusResponse {
    statuses: Vec<StatusResponse>,
}

#[derive(serde::Deserialize)]
struct StatusResponse {
    context: String,
    state: String,
    target_url: Option<String>,
}

#[derive(serde::Deserialize)]
struct CheckRunsResponse {
    check_runs: Vec<CheckRunResponse>,
}

#[derive(serde::Deserialize)]
struct CheckRunResponse {
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
}

impl GithubOauthImpl {
    /// GET `path` of the REST API as the user, read as `T`.
    async fn read<T: DeserializeOwned>(
        &self,
        access_token: &str,
        path: &str,
        query: &[(&str, &str)],
        context: &str,
    ) -> Result<T, GithubError> {
        let response = self
            .client
            .get(format!("{}{path}", self.api_base_url()))
            .query(query)
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
            return response.json().await.map_err(transport_error);
        }
        let headers = response.headers().clone();
        let body = response_text(response).await;
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(status, &headers, body, context));
        }
        Err(refusal(status).unwrap_or_else(|| unexpected(status, &headers, body, context)))
    }

    /// The repository's default branch, and whether the user may push to
    /// it; `None` when they cannot reach it.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_repository_access(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
    ) -> Result<Option<GithubRepositoryAccess>, GithubError> {
        let read: Result<RepositoryResponse, GithubError> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}"),
                &[],
                "failed to read repository",
            )
            .await;
        match read {
            Ok(repository) => Ok(Some(GithubRepositoryAccess {
                default_branch: repository.default_branch,
                push: repository
                    .permissions
                    .is_some_and(|permissions| permissions.push),
            })),
            // GitHub answers a private repository the user cannot see as
            // missing.
            Err(GithubError::RepositoryUnavailable) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The newest pull request, open or closed, from the repository's own
    /// branch `head`, with its head commit.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn find_pull_request_by_head(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        head: &str,
    ) -> Result<Option<GithubPullRequestHead>, GithubError> {
        // GitHub names a head as `owner:branch`; newest first, so a branch
        // whose pull request was closed and opened again reads the open one.
        let pull_requests: Vec<PullRequestListItem> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/pulls"),
                &[
                    ("head", format!("{owner}:{head}").as_str()),
                    ("state", "all"),
                    ("sort", "created"),
                    ("direction", "desc"),
                    ("per_page", "1"),
                ],
                "failed to find pull request",
            )
            .await?;
        Ok(pull_requests
            .into_iter()
            .next()
            .map(|pull_request| GithubPullRequestHead {
                number: pull_request.number,
                html_url: pull_request.html_url,
                state: pull_request.state,
                merged: pull_request.merged_at.is_some(),
                head_sha: pull_request.head.sha,
            }))
    }

    /// The check suites on the commit `sha`.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_check_suites(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<GithubCheckSuite>, GithubError> {
        let suites: CheckSuitesResponse = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}/check-suites"),
                &[("per_page", PAGE)],
                "failed to list check suites",
            )
            .await?;
        Ok(suites
            .check_suites
            .into_iter()
            .map(|suite| GithubCheckSuite {
                id: suite.id,
                app: suite
                    .app
                    .and_then(|app| app.slug)
                    .unwrap_or_else(|| "unknown".to_string()),
                status: suite.status.unwrap_or_else(|| "queued".to_string()),
                conclusion: suite.conclusion,
                check_runs: suite.latest_check_runs_count,
            })
            .collect())
    }

    /// The latest commit status of each context on the commit `sha`.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_commit_statuses(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<GithubCommitStatus>, GithubError> {
        let combined: CombinedStatusResponse = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}/status"),
                &[("per_page", PAGE)],
                "failed to read commit status",
            )
            .await?;
        Ok(combined
            .statuses
            .into_iter()
            .map(|status| GithubCommitStatus {
                context: status.context,
                state: status.state,
                target_url: status.target_url,
            })
            .collect())
    }

    /// The check runs on the commit `sha`.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_check_runs(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<GithubPullRequestCheckRun>, GithubError> {
        let runs: CheckRunsResponse = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}/check-runs"),
                &[("per_page", PAGE)],
                "failed to list check runs",
            )
            .await?;
        Ok(runs
            .check_runs
            .into_iter()
            .map(|run| GithubPullRequestCheckRun {
                id: run.id,
                name: run.name,
                status: run.status,
                conclusion: run.conclusion,
                url: run.html_url,
                started_at: None,
                completed_at: None,
            })
            .collect())
    }

    /// Opens a pull request as the user. GitHub declining it is a
    /// [`GithubOpenPullRequestOutcome::Rejected`] value, not an error.
    #[tracing::instrument(skip(self, access_token, pull_request), fields(head = %pull_request.head, base = %pull_request.base), err)]
    pub async fn open_pull_request(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        pull_request: &GithubOpenPullRequest,
    ) -> Result<GithubOpenPullRequestOutcome, GithubError> {
        #[derive(serde::Serialize)]
        struct OpenRequest<'a> {
            title: &'a str,
            body: &'a str,
            head: &'a str,
            base: &'a str,
        }

        #[derive(serde::Deserialize)]
        struct Opened {
            number: u64,
        }

        let response = self
            .client
            .post(format!(
                "{}/repos/{owner}/{repo}/pulls",
                self.api_base_url()
            ))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&OpenRequest {
                title: &pull_request.title,
                body: &pull_request.body,
                head: &pull_request.head,
                base: &pull_request.base,
            })
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if status.is_success() {
            let opened: Opened = response.json().await.map_err(transport_error)?;
            return Ok(GithubOpenPullRequestOutcome::Opened(opened.number));
        }

        let headers = response.headers().clone();
        let body = response_text(response).await;
        // A throttled request is a 403 too; it must not read as a refusal.
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(
                status,
                &headers,
                body,
                "failed to open pull request",
            ));
        }
        match status {
            // A token that no longer works, or a user who may not reach the
            // repository, is the same refusal on every try.
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
                Err(refusal(status).unwrap_or(GithubError::Forbidden))
            }
            // GitHub's answer about the pull request itself: one already
            // open, or nothing to propose.
            StatusCode::UNPROCESSABLE_ENTITY => {
                let message = serde_json::from_str::<GithubErrorBody>(&body)
                    .map(GithubErrorBody::reason)
                    .unwrap_or_else(|_| "GitHub declined to open the pull request.".to_string());
                Ok(GithubOpenPullRequestOutcome::Rejected(message))
            }
            _ => Err(unexpected(
                status,
                &headers,
                body,
                "failed to open pull request",
            )),
        }
    }
}
