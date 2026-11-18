//! The verify Phase a repository declares for the `code` Flow's `accept`
//! Step, before its merge: `.macro/verify.json` at the root of the Run's
//! base branch, beside the checks file.
//!
//! ```json
//! { "verify": true }
//! ```
//!
//! `verify` is whether the Phase runs at all, `true` when left out: with
//! `false`, the `accept` Step passes the change on unchecked. A repository
//! with no file is checked. A file with any other key, such as the `deploy`
//! key an earlier factory read, does not read, and turns nothing off: the
//! factory ends a Run at its merge, and waits for no deploy.
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

/// The verify Phase a repository declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyFile {
    /// Whether the Phase runs.
    #[serde(default = "on")]
    pub verify: bool,
}

fn on() -> bool {
    true
}

impl Default for VerifyFile {
    /// A repository with no verify file: checked before its merge.
    fn default() -> Self {
        Self { verify: true }
    }
}

/// Why a repository's verify file does not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyFileError {
    /// The file is not the verify Phase's JSON.
    #[error("`{VERIFY_FILE}` does not read ({0})")]
    Unreadable(String),
    /// The file is larger than one is read.
    #[error("`{VERIFY_FILE}` does not read (it is over {MAX_FILE_BYTES} bytes)")]
    TooLarge,
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
        serde_json::from_str(text).map_err(|error| VerifyFileError::Unreadable(error.to_string()))
    }
}
