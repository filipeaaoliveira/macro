//! The text of a pull request as the user: its description, read and
//! replaced, and its conversation's comments, listed, added and edited.
//!
//! Refusals read as in [`super::pull_requests`]: 401, 403 that is not
//! throttling, and 404 are refusals; throttling, 5xx and transport failures
//! are [`GithubError::Upstream`].

use reqwest::Method;

use super::pull_requests::{is_repository_part, refusal};
use super::{
    GithubOauthImpl, REQUEST_TIMEOUT, USER_AGENT, response_text, throttling, transport_error,
    unexpected,
};
use crate::domain::models::{GithubError, GithubListing};

#[cfg(test)]
mod test;

/// A comment in a pull request's conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubIssueComment {
    /// GitHub's id of the comment, which edits it.
    pub id: u64,
    /// Its text; empty when GitHub gives none.
    pub body: String,
}

#[derive(serde::Deserialize)]
struct PullRequestBody {
    #[serde(default)]
    body: Option<String>,
}

#[derive(serde::Deserialize)]
struct IssueCommentResponse {
    id: u64,
    #[serde(default)]
    body: Option<String>,
}

/// The refusal of a request whose owner or repository is not a name GitHub
/// gives: it is never sent.
fn checked(owner: &str, repo: &str) -> Result<(), GithubError> {
    if is_repository_part(owner) && is_repository_part(repo) {
        return Ok(());
    }
    Err(GithubError::Internal(anyhow::anyhow!(
        "not a repository: {owner:?}/{repo:?}"
    )))
}

impl GithubOauthImpl {
    /// Send `body` as `{"body": ...}` to `path` with `method`, as the user.
    async fn send_text(
        &self,
        access_token: &str,
        method: Method,
        path: &str,
        body: &str,
        context: &str,
    ) -> Result<(), GithubError> {
        #[derive(serde::Serialize)]
        struct Text<'a> {
            body: &'a str,
        }

        let response = self
            .client
            .request(method, format!("{}{path}", self.api_base_url()))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(&Text { body })
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let headers = response.headers().clone();
        let text = response_text(response).await;
        // A throttled write is a 403 too; it must not read as a refusal.
        if throttling(status, &headers, &text).is_some() {
            return Err(unexpected(status, &headers, text, context));
        }
        Err(refusal(status).unwrap_or_else(|| unexpected(status, &headers, text, context)))
    }

    /// The description of pull request `number`; none when it has none.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn pull_request_body(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<Option<String>, GithubError> {
        checked(owner, repo)?;
        let read: PullRequestBody = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/pulls/{number}"),
                &[],
                "failed to read a pull request's description",
            )
            .await?;
        Ok(read.body)
    }

    /// Replace the description of pull request `number` with `body`.
    #[tracing::instrument(skip(self, access_token, body), err)]
    pub async fn edit_pull_request_body(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), GithubError> {
        checked(owner, repo)?;
        self.send_text(
            access_token,
            Method::PATCH,
            &format!("/repos/{owner}/{repo}/pulls/{number}"),
            body,
            "failed to edit a pull request's description",
        )
        .await
    }

    /// The comments in the conversation of pull request `number`, oldest
    /// first, as many pages as are read.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_issue_comments(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> Result<GithubListing<GithubIssueComment>, GithubError> {
        checked(owner, repo)?;
        self.read_pages(
            access_token,
            &format!("/repos/{owner}/{repo}/issues/{number}/comments"),
            "failed to list a pull request's comments",
            |comments: Vec<IssueCommentResponse>| {
                comments
                    .into_iter()
                    .map(|comment| GithubIssueComment {
                        id: comment.id,
                        body: comment.body.unwrap_or_default(),
                    })
                    .collect()
            },
        )
        .await
    }

    /// Add `body` to the conversation of pull request `number`.
    #[tracing::instrument(skip(self, access_token, body), err)]
    pub async fn create_issue_comment(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<(), GithubError> {
        checked(owner, repo)?;
        self.send_text(
            access_token,
            Method::POST,
            &format!("/repos/{owner}/{repo}/issues/{number}/comments"),
            body,
            "failed to comment on a pull request",
        )
        .await
    }

    /// Replace the text of the comment `comment_id` with `body`.
    #[tracing::instrument(skip(self, access_token, body), err)]
    pub async fn edit_issue_comment(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        comment_id: u64,
        body: &str,
    ) -> Result<(), GithubError> {
        checked(owner, repo)?;
        self.send_text(
            access_token,
            Method::PATCH,
            &format!("/repos/{owner}/{repo}/issues/comments/{comment_id}"),
            body,
            "failed to edit a pull request's comment",
        )
        .await
    }
}
