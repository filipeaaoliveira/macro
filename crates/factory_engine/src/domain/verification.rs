//! The verify Phase a repository declares for the `code` Flow, after its
//! merge: `.macro/verify.json` at the root of the Run's base branch, beside
//! the checks file.
//!
//! ```json
//! {
//!   "verify": true,
//!   "deploy": { "environment": "production" }
//! }
//! ```
//!
//! `verify` is whether the Phase runs at all, `true` when left out: with
//! `false`, the `accept` Step passes the change on unchecked and the `deploy`
//! Step ends the Run once it merged. `deploy` names the signal the Run waits
//! for once it merged: a GitHub deployment of the merge commit to
//! `environment` whose newest status is `success`. A repository that names
//! none, or has no file, deploys nothing the Run waits for: the `verify`
//! Step checks the merged commit itself.
//!
//! The file is read from the base branch, as the checks file is. A Run's
//! change to it is a Control File (`control_files`), so a change that turns
//! the Phase off is Tampered and merges only once a person approved it.

#[cfg(test)]
mod test;

use serde::Deserialize;

/// Where a repository declares its verify Phase, from its root.
pub const VERIFY_FILE: &str = ".macro/verify.json";

/// The largest verify file read, in bytes.
const MAX_FILE_BYTES: usize = 4096;

/// The longest environment name GitHub takes.
const MAX_ENVIRONMENT_BYTES: usize = 255;

/// The verify Phase a repository declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyFile {
    /// Whether the Phase runs.
    #[serde(default = "on")]
    pub verify: bool,
    /// What a merged Run waits for before it verifies; none when nothing
    /// deploys it.
    #[serde(default)]
    pub deploy: Option<DeploySignal>,
}

fn on() -> bool {
    true
}

impl Default for VerifyFile {
    /// A repository with no verify file: verified, on its merged commit.
    fn default() -> Self {
        Self {
            verify: true,
            deploy: None,
        }
    }
}

/// The signal a merged change was deployed: a GitHub deployment of the
/// merge commit to an environment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeploySignal {
    /// The GitHub environment the deployment is to, as the repository's
    /// deploy names it.
    pub environment: String,
}

/// Why a repository's verify file does not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyFileError {
    /// The file is not the verify Phase's JSON.
    #[error("{VERIFY_FILE} does not read: {0}")]
    Unreadable(String),
    /// The file is larger than one is read.
    #[error("{VERIFY_FILE} is over {MAX_FILE_BYTES} bytes")]
    TooLarge,
    /// The deploy's environment is blank or too long.
    #[error("{VERIFY_FILE} names a deploy environment that is blank or over {MAX_ENVIRONMENT_BYTES} bytes")]
    Environment,
}

impl VerifyFile {
    /// Read the verify Phase from the text of the repository's verify
    /// file; none, the default, when the base has no such file.
    pub fn read(text: Option<&str>) -> Result<Self, VerifyFileError> {
        let Some(text) = text else {
            return Ok(Self::default());
        };
        if text.len() > MAX_FILE_BYTES {
            return Err(VerifyFileError::TooLarge);
        }
        let file: Self = serde_json::from_str(text)
            .map_err(|error| VerifyFileError::Unreadable(error.to_string()))?;
        if let Some(deploy) = &file.deploy {
            let environment = deploy.environment.trim();
            if environment.is_empty() || deploy.environment.len() > MAX_ENVIRONMENT_BYTES {
                return Err(VerifyFileError::Environment);
            }
        }
        Ok(file)
    }
}
