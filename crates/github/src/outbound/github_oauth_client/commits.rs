//! Writes of a new branch as the user: one commit, on top of a commit the
//! repository has, that writes a few text files, and the branch that points
//! at it. A branch already there is never moved.

use reqwest::StatusCode;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::pull_requests::{is_branch_name, refusal};
use super::{
    GithubErrorBody, GithubOauthImpl, REQUEST_TIMEOUT, USER_AGENT, response_text, throttling,
    transport_error, unexpected,
};
use crate::domain::models::GithubError;
use github_pull_requests::domain::models::{is_github_owner, is_github_repository_name};

#[cfg(test)]
mod test;

/// What GitHub answers creating a ref that is already there with.
const REFERENCE_EXISTS: &str = "Reference already exists";

/// The mode of a regular, not executable, file in a git tree.
const FILE_MODE: &str = "100644";

/// What asking GitHub to create a branch came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubBranchCreation {
    /// GitHub created it, at the commit it made.
    Created,
    /// A branch of that name is already there; it was left as it is.
    Exists,
    /// GitHub declined to create it, for this reason, as a ruleset that
    /// restricts which branches may be created does.
    Refused(String),
}

/// A branch to create: one commit on top of `base_commit` that writes
/// `files`.
#[derive(Debug, Clone, Copy)]
pub struct GithubNewBranch<'a> {
    /// Its name.
    pub name: &'a str,
    /// The full id of the commit its one commit goes on top of.
    pub base_commit: &'a str,
    /// That commit's message.
    pub message: &'a str,
    /// The files it writes: each path from the repository's root, and the
    /// file's whole text.
    pub files: &'a [(&'a str, &'a str)],
}

#[derive(serde::Deserialize)]
struct GitObject {
    sha: String,
}

#[derive(serde::Deserialize)]
struct GitCommit {
    tree: GitObject,
}

impl GithubOauthImpl {
    /// Creates `branch` as the user, at a new commit on top of its base
    /// commit that writes each of its files and leaves every other file as
    /// the base commit has it. The branch is only ever created: one already
    /// there, the default branch included, is left as it is
    /// ([`GithubBranchCreation::Exists`]). A retry after a lost answer finds
    /// the branch there; the commit it made again is left unreferenced.
    /// An owner, repository, branch or commit GitHub would not take is
    /// never sent.
    #[tracing::instrument(skip(self, access_token, branch), fields(branch = branch.name), err)]
    pub async fn create_branch_with_files(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: GithubNewBranch<'_>,
    ) -> Result<GithubBranchCreation, GithubError> {
        let GithubNewBranch {
            name: branch,
            base_commit,
            message,
            files,
        } = branch;
        if !is_github_owner(owner) || !is_github_repository_name(repo) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a repository: {owner:?}/{repo:?}"
            )));
        }
        if !is_branch_name(branch) {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a branch name: {branch:?}"
            )));
        }
        let is_commit_id =
            base_commit.len() == 40 && base_commit.chars().all(|c| c.is_ascii_hexdigit());
        if !is_commit_id {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "not a commit: {base_commit:?}"
            )));
        }

        // A new tree is the base commit's with the files written over it;
        // the commit holding it has the base commit as its one parent.
        let base: GitCommit = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/git/commits/{base_commit}"),
                &[],
                "failed to read a commit",
            )
            .await?;
        let entries: Vec<serde_json::Value> = files
            .iter()
            .map(|(path, text)| {
                serde_json::json!({
                    "path": path,
                    "mode": FILE_MODE,
                    "type": "blob",
                    "content": text,
                })
            })
            .collect();
        let tree: GitObject = self
            .write(
                access_token,
                &format!("/repos/{owner}/{repo}/git/trees"),
                &serde_json::json!({ "base_tree": base.tree.sha, "tree": entries }),
                "failed to make a tree",
            )
            .await?;
        let commit: GitObject = self
            .write(
                access_token,
                &format!("/repos/{owner}/{repo}/git/commits"),
                &serde_json::json!({
                    "message": message,
                    "tree": tree.sha,
                    "parents": [base_commit],
                }),
                "failed to make a commit",
            )
            .await?;

        // Creating a ref never moves one: GitHub refuses a name already
        // taken.
        let response = self
            .post(
                access_token,
                &format!("/repos/{owner}/{repo}/git/refs"),
                &serde_json::json!({ "ref": format!("refs/heads/{branch}"), "sha": commit.sha }),
            )
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(GithubBranchCreation::Created);
        }
        let headers = response.headers().clone();
        let body = response_text(response).await;
        // GitHub's answer about the branch itself: the name is taken, or a
        // rule of the repository forbids it.
        if status == StatusCode::UNPROCESSABLE_ENTITY {
            let Ok(error) = serde_json::from_str::<GithubErrorBody>(&body) else {
                return Err(unexpected(
                    status,
                    &headers,
                    body,
                    "failed to create a branch",
                ));
            };
            if error.message == REFERENCE_EXISTS {
                return Ok(GithubBranchCreation::Exists);
            }
            return Ok(GithubBranchCreation::Refused(error.reason()));
        }
        Err(refused(status, &headers, body, "failed to create a branch"))
    }

    /// POSTs `body` to `path` as the user and reads GitHub's answer as `T`.
    async fn write<B: Serialize, T: DeserializeOwned>(
        &self,
        access_token: &str,
        path: &str,
        body: &B,
        context: &str,
    ) -> Result<T, GithubError> {
        let response = self.post(access_token, path, body).await?;
        let status = response.status();
        if status.is_success() {
            return response.json().await.map_err(transport_error);
        }
        let headers = response.headers().clone();
        let body = response_text(response).await;
        Err(refused(status, &headers, body, context))
    }

    /// POSTs `body` to `path` as the user.
    async fn post<B: Serialize>(
        &self,
        access_token: &str,
        path: &str,
        body: &B,
    ) -> Result<reqwest::Response, GithubError> {
        self.client
            .post(format!("{}{path}", self.api_base_url()))
            .header("Authorization", format!("Bearer {access_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .header("X-GitHub-Api-Version", "2022-11-28")
            .json(body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(transport_error)
    }
}

/// A write GitHub did not take: throttling is an outage, a token or access
/// GitHub refuses is that refusal, anything else is unexpected.
fn refused(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    body: String,
    context: &str,
) -> GithubError {
    if throttling(status, headers, &body).is_some() {
        return unexpected(status, headers, body, context);
    }
    refusal(status).unwrap_or_else(|| unexpected(status, headers, body, context))
}
