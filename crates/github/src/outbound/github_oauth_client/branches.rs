//! Reads of what a branch holds as the user: its head, a file at its head,
//! and what it changed against another branch.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use reqwest::StatusCode;

use super::{
    GithubOauthImpl, REQUEST_TIMEOUT, USER_AGENT, response_text, throttling, transport_error,
    unexpected,
};
use crate::domain::models::GithubError;

#[cfg(test)]
mod test;

/// The most files GitHub lists when it compares two commits.
const COMPARE_FILES_LISTED: usize = 300;

#[derive(serde::Deserialize)]
struct ContentResponse {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    encoding: Option<String>,
    #[serde(default)]
    content: Option<String>,
}

#[derive(serde::Deserialize)]
struct BranchHeadResponse {
    commit: BranchCommit,
}

#[derive(serde::Deserialize)]
struct BranchCommit {
    sha: String,
}

#[derive(serde::Deserialize)]
struct CommitResponse {
    parents: Vec<BranchCommit>,
}

#[derive(serde::Deserialize)]
struct CompareStatus {
    status: String,
}

/// Whether `text` is a commit's full id: 40 hexadecimal digits.
fn is_commit_id(text: &str) -> bool {
    text.len() == 40 && text.chars().all(|c| c.is_ascii_hexdigit())
}

#[derive(serde::Deserialize)]
struct CompareMergeBase {
    merge_base_commit: BranchCommit,
}

#[derive(serde::Deserialize)]
struct CompareResponse {
    #[serde(default)]
    files: Option<Vec<CompareFile>>,
}

#[derive(serde::Deserialize)]
struct CompareFile {
    filename: String,
    #[serde(default)]
    previous_filename: Option<String>,
    #[serde(default)]
    patch: Option<String>,
}

/// A file one commit changed against another, with what changed in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparedFile {
    /// Its path at the later commit.
    pub path: String,
    /// Its path at the earlier one, when it was renamed.
    pub previous_path: Option<String>,
    /// Its unified diff's hunks; none when GitHub gives none, as for a
    /// binary file or one too large to show.
    pub patch: Option<String>,
}

impl GithubOauthImpl {
    /// The commit at the head of `branch`; `None` when the user reaches no
    /// such branch, as when it was never pushed, or it or its repository
    /// is gone.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_branch_head(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<Option<String>, GithubError> {
        let branch = urlencoding::encode(branch);
        let read: Result<BranchHeadResponse, GithubError> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/branches/{branch}"),
                &[],
                "failed to read a branch",
            )
            .await;
        match read {
            Ok(branch) => Ok(Some(branch.commit.sha)),
            Err(GithubError::RepositoryUnavailable) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Delete `branch` as the user. True once it is gone: deleted now, or
    /// already gone (GitHub answers a missing branch 422). A repository the
    /// user cannot reach is [`GithubError::RepositoryUnavailable`].
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn delete_branch(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> Result<bool, GithubError> {
        let branch = urlencoding::encode(branch);
        let response = self
            .client
            .delete(format!(
                "{}/repos/{owner}/{repo}/git/refs/heads/{branch}",
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
        if status.is_success() || status == StatusCode::UNPROCESSABLE_ENTITY {
            return Ok(true);
        }
        let headers = response.headers().clone();
        let body = response_text(response).await;
        if throttling(status, &headers, &body).is_some() {
            return Err(unexpected(
                status,
                &headers,
                body,
                "failed to delete a branch",
            ));
        }
        match status {
            StatusCode::UNAUTHORIZED => Err(GithubError::ReauthenticationRequired),
            StatusCode::FORBIDDEN => Err(GithubError::Forbidden),
            StatusCode::NOT_FOUND => Err(GithubError::RepositoryUnavailable),
            _ => Err(unexpected(
                status,
                &headers,
                body,
                "failed to delete a branch",
            )),
        }
    }

    /// The text of the file at `path` on `branch`, or at a commit when
    /// `branch` is a commit's full id; `None` when it holds no file there (a
    /// folder there is not one). A branch, commit or repository the user
    /// cannot reach is [`GithubError::RepositoryUnavailable`], never a
    /// missing file.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_file_text(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        branch: &str,
        path: &str,
    ) -> Result<Option<String>, GithubError> {
        let read: Result<serde_json::Value, GithubError> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/contents/{path}"),
                &[("ref", branch)],
                "failed to read a file",
            )
            .await;
        let value = match read {
            Ok(value) => value,
            // GitHub answers a missing file, a missing branch or commit and
            // a repository out of reach alike: the branch's, or the
            // commit's, own read tells them apart.
            Err(GithubError::RepositoryUnavailable) => {
                let at = if is_commit_id(branch) {
                    format!("/repos/{owner}/{repo}/commits/{branch}")
                } else {
                    format!("/repos/{owner}/{repo}/branches/{branch}")
                };
                let _: serde_json::Value = self
                    .read(access_token, &at, &[], "failed to read a branch")
                    .await?;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };

        // A folder answers with a list of its entries, and a symlink or a
        // submodule with another type: neither is a file.
        if value.is_array() {
            return Ok(None);
        }
        let content: ContentResponse = serde_json::from_value(value).map_err(|error| {
            GithubError::Internal(anyhow::anyhow!(
                "GitHub's answer for the file {path} does not read: {error}"
            ))
        })?;
        if content.kind != "file" {
            return Ok(None);
        }

        // GitHub gives a file's content in base64, wrapped over lines.
        let (Some("base64"), Some(encoded)) = (content.encoding.as_deref(), content.content) else {
            return Err(GithubError::Internal(anyhow::anyhow!(
                "GitHub gave the file {path} with no base64 content"
            )));
        };
        let encoded: String = encoded.split_whitespace().collect();
        let bytes = STANDARD
            .decode(encoded)
            .map_err(|error| anyhow::anyhow!("file {path} is not base64: {error}"))?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| GithubError::Internal(anyhow::anyhow!("file {path} is not UTF-8 text")))
    }

    /// The files `head` changed against its merge base with `base`, with
    /// their hunks; `None` when GitHub lists only part of them.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn compare_files(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<Option<Vec<ComparedFile>>, GithubError> {
        let compared: CompareResponse = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/compare/{base}...{head}"),
                &[],
                "failed to compare commits",
            )
            .await?;
        let Some(files) = compared.files else {
            return Ok(None);
        };
        if files.len() >= COMPARE_FILES_LISTED {
            return Ok(None);
        }
        Ok(Some(
            files
                .into_iter()
                .map(|file| ComparedFile {
                    path: file.filename,
                    previous_path: file.previous_filename,
                    patch: file.patch,
                })
                .collect(),
        ))
    }

    /// The paths `head` changed against its merge base with `base`, a
    /// renamed file under both its names; `None` when GitHub lists only part
    /// of them.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn list_changed_paths(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<Option<Vec<String>>, GithubError> {
        let compared: CompareResponse = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/compare/{base}...{head}"),
                &[],
                "failed to compare branches",
            )
            .await?;
        let Some(files) = compared.files else {
            return Ok(None);
        };
        if files.len() >= COMPARE_FILES_LISTED {
            return Ok(None);
        }

        let mut paths = Vec::new();
        for file in files {
            paths.extend(file.previous_filename);
            paths.push(file.filename);
        }
        Ok(Some(paths))
    }

    /// The parents of the commit `sha`, its first the branch it was made
    /// on; a commit or repository the user cannot reach is
    /// [`GithubError::RepositoryUnavailable`].
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn get_commit_parents(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<Vec<String>, GithubError> {
        let commit: CommitResponse = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/commits/{sha}"),
                &[],
                "failed to read a commit",
            )
            .await?;
        Ok(commit
            .parents
            .into_iter()
            .map(|parent| parent.sha)
            .collect())
    }

    /// Whether the commit `head` contains the commit `ancestor`: it is that
    /// commit, or comes after it.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn commit_contains(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        ancestor: &str,
        head: &str,
    ) -> Result<bool, GithubError> {
        let compared: CompareStatus = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/compare/{ancestor}...{head}"),
                &[("per_page", "1")],
                "failed to compare commits",
            )
            .await?;
        Ok(matches!(compared.status.as_str(), "ahead" | "identical"))
    }

    /// The best common ancestor of the commits `base` and `head`, both full
    /// commit ids: the newest commit both contain. `None` when they share
    /// no history, or one is not in the repository (GitHub answers 404),
    /// and for an id that is not a full commit id.
    #[tracing::instrument(skip(self, access_token), err)]
    pub async fn merge_base(
        &self,
        access_token: &str,
        owner: &str,
        repo: &str,
        base: &str,
        head: &str,
    ) -> Result<Option<String>, GithubError> {
        if !is_commit_id(base) || !is_commit_id(head) {
            return Ok(None);
        }
        let compared: Result<CompareMergeBase, GithubError> = self
            .read(
                access_token,
                &format!("/repos/{owner}/{repo}/compare/{base}...{head}"),
                &[("per_page", "1")],
                "failed to compare commits",
            )
            .await;
        match compared {
            Ok(compared) => Ok(Some(compared.merge_base_commit.sha)),
            Err(GithubError::RepositoryUnavailable) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
