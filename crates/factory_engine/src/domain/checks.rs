//! The checks a repository declares for the factory's write and fix Steps
//! to run on an agent's work: `.macro/checks.json` at the root of the Run's
//! branch.
//!
//! ```json
//! {
//!   "rounds": 3,
//!   "checks": [
//!     { "name": "web", "run": "bun run --cwd apps/web check", "paths": ["apps/web", "packages"] },
//!     { "name": "lint", "run": "just check", "timeout": 300 }
//!   ]
//! }
//! ```
//!
//! A check runs `run` from the repository's root. One with `paths` runs only
//! when the branch changed a file at one of them, or under one when it is a
//! folder; one without runs every time. `timeout` is how many seconds a
//! check the factory runs itself may take, from 1 to 1200 (20 minutes), 600
//! when left out; a Step starts it only with that much time left. `rounds` is how
//! many times a Step runs the checks at most, 3 when left out.

#[cfg(test)]
mod test;

use std::collections::BTreeSet;

use factory_records::domain::models::{MAX_CHECK_RESULT_OUTPUT_BYTES, MAX_CHECK_ROUNDS};
use serde::Deserialize;

/// Where a repository declares its checks, from its root.
pub const CHECKS_FILE: &str = ".macro/checks.json";

/// How many times a Step runs the checks when the file does not say.
const DEFAULT_ROUNDS: u8 = 3;

/// The most checks a repository declares.
const MAX_CHECKS: usize = 64;

/// The longest command a check runs, in bytes.
const MAX_RUN_BYTES: usize = 4096;

/// The longest time a check may take, in seconds.
pub const MAX_CHECK_TIMEOUT_SECS: u64 = 20 * 60;

/// The time a check may take when the file does not say, in seconds.
pub const DEFAULT_CHECK_TIMEOUT_SECS: u64 = 10 * 60;

/// The largest checks file read, in bytes: what a round's record keeps.
const MAX_FILE_BYTES: usize = 64 * 1024;

/// One check: a command run from the repository's root.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryCheck {
    /// Its name, unique among the repository's checks.
    pub name: String,
    /// The command, run from the repository's root.
    pub run: String,
    /// The files and folders, from the repository's root, whose change
    /// makes it run; none runs it every time.
    #[serde(default)]
    pub paths: Vec<String>,
    /// How many seconds it may take when the factory runs it itself.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_CHECK_TIMEOUT_SECS
}

impl RepositoryCheck {
    /// Whether a branch that changed `changed` runs it.
    fn applies_to(&self, changed: &[String]) -> bool {
        if self.paths.is_empty() {
            return true;
        }
        changed.iter().any(|path| {
            self.paths.iter().any(|declared| {
                let declared = declared.trim_end_matches('/');
                path == declared
                    || path
                        .strip_prefix(declared)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
        })
    }
}

/// The checks a repository declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryChecks {
    /// How many times a Step runs them at most.
    #[serde(default = "default_rounds")]
    pub rounds: u8,
    /// The checks, in the order they run.
    pub checks: Vec<RepositoryCheck>,
}

fn default_rounds() -> u8 {
    DEFAULT_ROUNDS
}

/// Why a repository's checks file does not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChecksFileError {
    /// The file is not the checks' JSON.
    #[error("{CHECKS_FILE} does not read: {0}")]
    Unreadable(String),
    /// `rounds` is out of its range.
    #[error("{CHECKS_FILE} asks for {0} rounds; a step runs from 1 to {MAX_CHECK_ROUNDS}")]
    Rounds(u8),
    /// It declares more checks than a Step runs.
    #[error("{CHECKS_FILE} declares {0} checks; a step runs at most {MAX_CHECKS}")]
    TooMany(usize),
    /// A check's name is empty, too long, or not of `[a-z0-9_-]`.
    #[error("{CHECKS_FILE} names a check {0:?}; a name is 1 to 64 of a-z, 0-9, _ and -")]
    Name(String),
    /// Two checks share a name.
    #[error("{CHECKS_FILE} names two checks {0}")]
    DuplicateName(String),
    /// A check names a path that is empty, not from the repository's root,
    /// or a pattern.
    #[error(
        "{CHECKS_FILE} gives check {check} the path {path:?}; a path is a file or folder from the repository's root, with no ., .. or pattern"
    )]
    Path {
        /// The check.
        check: String,
        /// The path.
        path: String,
    },
    /// The file is larger than a round keeps.
    #[error("{CHECKS_FILE} is over {MAX_FILE_BYTES} bytes")]
    TooLarge,
    /// A check's timeout is zero or past [`MAX_CHECK_TIMEOUT_SECS`].
    #[error(
        "{CHECKS_FILE} gives check {0} a timeout outside 1 to {MAX_CHECK_TIMEOUT_SECS} seconds"
    )]
    Timeout(String),
    /// A check's command is blank or too long.
    #[error("{CHECKS_FILE} gives check {0} a blank command or one over {MAX_RUN_BYTES} bytes")]
    Run(String),
}

impl RepositoryChecks {
    /// Read the checks from the text of the repository's checks file.
    pub fn parse(text: &str) -> Result<Self, ChecksFileError> {
        if text.len() > MAX_FILE_BYTES {
            return Err(ChecksFileError::TooLarge);
        }
        let checks: Self = serde_json::from_str(text)
            .map_err(|error| ChecksFileError::Unreadable(error.to_string()))?;
        if checks.rounds == 0 || checks.rounds > MAX_CHECK_ROUNDS {
            return Err(ChecksFileError::Rounds(checks.rounds));
        }
        if checks.checks.len() > MAX_CHECKS {
            return Err(ChecksFileError::TooMany(checks.checks.len()));
        }

        let mut names = BTreeSet::new();
        for check in &checks.checks {
            let name_valid = (1..=64).contains(&check.name.len())
                && check.name.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-')
                });
            if !name_valid {
                return Err(ChecksFileError::Name(check.name.clone()));
            }
            if !names.insert(check.name.as_str()) {
                return Err(ChecksFileError::DuplicateName(check.name.clone()));
            }
            if check.run.trim().is_empty() || check.run.len() > MAX_RUN_BYTES {
                return Err(ChecksFileError::Run(check.name.clone()));
            }
            if check.timeout == 0 || check.timeout > MAX_CHECK_TIMEOUT_SECS {
                return Err(ChecksFileError::Timeout(check.name.clone()));
            }
            if let Some(path) = check.paths.iter().find(|path| !valid_path(path)) {
                return Err(ChecksFileError::Path {
                    check: check.name.clone(),
                    path: path.clone(),
                });
            }
        }
        Ok(checks)
    }

    /// The checks a branch that changed `changed` runs, in order: every
    /// check when the changed paths are not known, and those named in
    /// `kept` whatever changed.
    pub fn for_changes(
        &self,
        changed: Option<&[String]>,
        kept: &[String],
    ) -> Vec<&RepositoryCheck> {
        self.checks
            .iter()
            .filter(|check| {
                kept.contains(&check.name)
                    || changed.is_none_or(|changed| check.applies_to(changed))
            })
            .collect()
    }
}

/// Whether `path` names a file or folder from the repository's root: not
/// empty, not absolute, no `.` or `..` part, and no pattern, which the
/// checks don't match.
fn valid_path(path: &str) -> bool {
    let path = path.strip_suffix('/').unwrap_or(path);
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['*', '?', '[', ']', '{', '}', '!', '\\'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The end of `text` a verified round keeps: without control characters
/// but line breaks and tabs, and at most [`MAX_CHECK_RESULT_OUTPUT_BYTES`]
/// once escaped as JSON, so a round's results stay within what their
/// record holds. Never split inside a character.
pub(crate) fn kept_output(text: &str) -> String {
    let mut kept = Vec::new();
    let mut size = 0;
    for character in text.chars().rev() {
        let escaped = match character {
            '"' | '\\' | '\n' | '\r' | '\t' => 2,
            character if character.is_control() => continue,
            character => character.len_utf8(),
        };
        if size + escaped > MAX_CHECK_RESULT_OUTPUT_BYTES {
            break;
        }
        size += escaped;
        kept.push(character);
    }
    kept.iter().rev().collect()
}
