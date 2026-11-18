//! Reads of what a branch holds as the user: its head, a file at its head,
//! and what it changed against another branch.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::GithubOauthImpl;
use crate::domain::models::GithubError;

#[cfg(test)]
mod test;

/// A file GitHub's contents API gives no content for: one over 1 MB. It
/// is the source of the [`GithubError::Internal`] that answers it, so a
/// caller can tell it apart.
#[derive(Debug, thiserror::Error)]
#[error("the file {path} is over the 1 MB GitHub's contents API gives")]
pub struct FileTooLarge {
    /// The file.
    pub path: String,
}

/// The most files GitHub lists when it compares two commits.
const COMPARE_FILES_LISTED: usize = 300;

#[derive(serde::Deserialize)]
struct ContentResponse {
    #[serde(rename = "type")]
    kind: String,
    /// The path of what GitHub answers with: a symlink's target's, when
    /// GitHub follows it to a file.
    #[serde(default)]
    path: Option<String>,
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
        // GitHub follows a symlink to the file it points at and answers with
        // that file: what is at `path` is a symlink, not a file.
        if content
            .path
            .as_deref()
            .is_some_and(|answered| answered != path)
        {
            return Ok(None);
        }

        // GitHub gives a file's content in base64, wrapped over lines; a file
        // over the 1 MB its contents API gives comes with none (`none`).
        let (Some("base64"), Some(encoded)) = (content.encoding.as_deref(), content.content) else {
            return Err(GithubError::Internal(anyhow::Error::new(FileTooLarge {
                path: path.to_string(),
            })));
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
