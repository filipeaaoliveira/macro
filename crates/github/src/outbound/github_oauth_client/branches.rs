//! Reads of what a branch holds as the user: a file at its head, and the
//! paths it changed against another branch.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::GithubOauthImpl;
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
struct CompareResponse {
    #[serde(default)]
    files: Option<Vec<CompareFile>>,
}

#[derive(serde::Deserialize)]
struct CompareFile {
    filename: String,
    #[serde(default)]
    previous_filename: Option<String>,
}

impl GithubOauthImpl {
    /// The text of the file at `path` on `branch`; `None` when the branch
    /// holds no file there (a folder there is not one). A branch or
    /// repository the user cannot reach is
    /// [`GithubError::RepositoryUnavailable`], never a missing file.
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
            // GitHub answers a missing file, a missing branch and a
            // repository out of reach alike: the branch's own read tells
            // them apart.
            Err(GithubError::RepositoryUnavailable) => {
                let _: serde_json::Value = self
                    .read(
                        access_token,
                        &format!("/repos/{owner}/{repo}/branches/{branch}"),
                        &[],
                        "failed to read a branch",
                    )
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
}
