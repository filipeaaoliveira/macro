//! Control Files: the files that decide whether a repository's checks pass,
//! such as its tests, lint and test configuration, CI workflows, package
//! scripts, agent instructions and its checks file.
//!
//! An agent that changes one can make failing work pass, so a `code` Run
//! whose branch changes a Control File its request does not ask for is
//! Tampered: the pull request Steps record the files on the Run, and the
//! Run merges only once a person approves them on the merge Gate.
//!
//! The request asks for a file when it names the file's path from the
//! repository's root, as `.github/workflows/ci.yml`. A pattern or a folder
//! in the request asks for nothing: a person approves those on the Gate.
//!
//! Which files are Control Files is a list of glob patterns, matched
//! against each changed path from the repository's root: `*` stays within
//! one folder, `**` spans any number of them, and `**/name` also matches
//! `name` at the root. [`DEFAULT_CONTROL_FILES`] is the list a Run's owner
//! has until they set their own.

#[cfg(test)]
mod test;

use factory_records::domain::models::Tampering;
use glob::{MatchOptions, Pattern};

/// The Control File patterns a Run's owner has until they set their own.
pub const DEFAULT_CONTROL_FILES: &[&str] = &[
    // CI workflows.
    ".github/workflows/**",
    ".github/actions/**",
    ".gitlab-ci.yml",
    ".circleci/**",
    // The repository's own checks, which the factory's checks rounds run.
    ".macro/checks.json",
    // Agent instructions.
    "**/AGENTS.md",
    "**/CLAUDE.md",
    // Package scripts.
    "**/package.json",
    "**/justfile",
    "**/Makefile",
    // Test and lint configuration.
    "**/biome.json",
    "**/biome.jsonc",
    "**/.eslintrc*",
    "**/eslint.config.*",
    "**/tsconfig*.json",
    "**/vitest.config.*",
    "**/jest.config.*",
    "**/playwright.config.*",
    "**/clippy.toml",
    "**/rustfmt.toml",
    "**/.rustfmt.toml",
    "**/.cargo/config.toml",
    "**/pyproject.toml",
    "**/pytest.ini",
    "**/ruff.toml",
    "**/.pre-commit-config.yaml",
    // The tests themselves.
    "**/test/**",
    "**/tests/**",
    "**/__tests__/**",
    "**/*.test.*",
    "**/*.spec.*",
    "**/test.rs",
    "**/*_test.go",
    "**/test_*.py",
];

/// The most patterns a person's list holds.
pub const MAX_PATTERNS: usize = 200;

/// The longest pattern, in bytes.
pub const MAX_PATTERN_BYTES: usize = 256;

/// Why a list of Control File patterns is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ControlFilesError {
    /// The list holds more than [`MAX_PATTERNS`] patterns.
    #[error("at most {MAX_PATTERNS} control file patterns")]
    TooMany,
    /// A pattern is empty, longer than [`MAX_PATTERN_BYTES`], starts at
    /// `/`, or is not a glob pattern.
    #[error("{0:?} is not a glob pattern of a path from the repository's root")]
    InvalidPattern(String),
}

/// A list of Control File patterns, ready to match changed paths.
#[derive(Debug, Clone)]
pub struct ControlFiles {
    patterns: Vec<Pattern>,
}

impl ControlFiles {
    /// The patterns of `patterns`, each checked.
    pub fn parse(patterns: &[String]) -> Result<Self, ControlFilesError> {
        if patterns.len() > MAX_PATTERNS {
            return Err(ControlFilesError::TooMany);
        }

        let parsed = patterns
            .iter()
            .map(|pattern| {
                let usable = !pattern.is_empty()
                    && pattern.len() <= MAX_PATTERN_BYTES
                    && !pattern.starts_with('/');
                usable
                    .then(|| Pattern::new(pattern).ok())
                    .flatten()
                    .ok_or_else(|| ControlFilesError::InvalidPattern(pattern.clone()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { patterns: parsed })
    }

    /// The factory's default list.
    pub fn default_patterns() -> Vec<String> {
        DEFAULT_CONTROL_FILES
            .iter()
            .map(|pattern| (*pattern).to_string())
            .collect()
    }

    /// Whether the list names no file.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `path`, from the repository's root, is a Control File.
    pub fn matches(&self, path: &str) -> bool {
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        self.patterns
            .iter()
            .any(|pattern| pattern.matches_with(path, options))
    }

    /// What a branch that changed `changed` did to the Control Files that
    /// `request` does not ask for: none when it changed none of them, and
    /// unlisted when GitHub could not list the changes (`None`).
    pub fn tampering(&self, changed: Option<&[String]>, request: &str) -> Option<Tampering> {
        if self.is_empty() {
            return None;
        }
        let Some(changed) = changed else {
            return Some(Tampering {
                files: Vec::new(),
                unlisted: true,
            });
        };

        let mut files: Vec<String> = Vec::new();
        for path in changed {
            let tampered = self.matches(path) && !names(request, path);
            if tampered && !files.contains(path) {
                files.push(path.clone());
            }
        }
        (!files.is_empty()).then_some(Tampering {
            files,
            unlisted: false,
        })
    }
}

/// Whether `text` names `path` whole: not as part of a longer path or word.
/// A dot right after it ends a sentence unless more of a path follows.
fn names(text: &str, path: &str) -> bool {
    let in_path = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '/' | '.');
    text.match_indices(path).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let mut after = text[at + path.len()..].chars();
        let starts = before.is_none_or(|c| !in_path(c));
        let ends = match after.next() {
            None => true,
            Some('.') => after.next().is_none_or(|c| !in_path(c)),
            Some(c) => !in_path(c),
        };
        starts && ends
    })
}
