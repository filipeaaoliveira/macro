//! Control Files: the files that decide whether a repository's checks pass,
//! such as its tests, lint and test configuration, CI workflows, package
//! scripts, agent instructions, its checks file and its verify file.
//!
//! An agent that changes one can make failing work pass, so a `code` Run
//! whose branch changes a Control File its request does not ask for is
//! Tampered: the pull request Steps record the files on the Run, and the
//! Run merges only once a person approves them on the merge Gate.
//!
//! The request asks for a file when it names the file's path from the
//! repository's root, as `.github/workflows/ci.yml`, and only when the Run's
//! owner created it in their own signed-in session: an agent that starts a
//! Run could otherwise name the file it means to weaken. A pattern or a
//! folder in the request asks for nothing: a person approves those on the
//! Gate. The files a request asks for are kept too, and the Gate lists them
//! as named by the request.
//!
//! Which files are Control Files is a list of glob patterns, matched
//! against each changed path from the repository's root, ignoring case:
//! `*` stays within one folder, `**` spans any number of them, and
//! `**/name` also matches `name` at the root. [`DEFAULT_CONTROL_FILES`] is
//! the list a team has until its admins set their own. A Run keeps the
//! list its owner's team had when it was created, so a change to the
//! team's list later does not change a Run under way.
//!
//! A path says nothing of what a change does inside a file, so a check
//! weakened in ordinary source, such as a lint suppression, an ignored
//! test or a type check turned off on one line, is not caught here: the
//! review sees those.

#[cfg(test)]
mod test;

use factory_records::domain::models::ControlFileChanges;
use glob::{MatchOptions, Pattern};

/// The Control File patterns a team has until its admins set their own.
pub const DEFAULT_CONTROL_FILES: &[&str] = &[
    // CI workflows.
    ".github/workflows/**",
    ".github/actions/**",
    ".gitlab-ci.yml",
    ".circleci/**",
    // The repository's own checks, which the factory's checks rounds run,
    // its verify Phase, which a change could turn off, and its setup, which
    // runs before every agent session and checks round.
    ".macro/checks.json",
    ".macro/verify.json",
    ".macro/setup.json",
    // Agent instructions and their tools' settings.
    "**/AGENTS.md",
    "**/CLAUDE.md",
    ".claude/**",
    ".agents/**",
    ".cursor/**",
    // Package and build scripts.
    "**/package.json",
    "**/justfile",
    "**/Makefile",
    "**/GNUmakefile",
    "**/Cargo.toml",
    "**/build.rs",
    "rust-toolchain*",
    "scripts/**",
    ".npmrc",
    "pnpm-workspace.yaml",
    "turbo.json",
    // Test and lint configuration.
    ".config/**",
    "**/biome*.json*",
    "**/.eslintrc*",
    "**/eslint.config.*",
    "**/tsconfig*.json",
    "**/vite.config.*",
    "**/vitest.config.*",
    "**/vitest.workspace.*",
    "**/jest.config.*",
    "**/playwright.config.*",
    "**/clippy.toml",
    "**/rustfmt.toml",
    "**/.rustfmt.toml",
    "**/.cargo/config.toml",
    "deny.toml",
    "**/pyproject.toml",
    "**/pytest.ini",
    "**/conftest.py",
    "**/ruff.toml",
    "**/.pre-commit-config.yaml",
    // The tests themselves, and what they compare against.
    "**/test/**",
    "**/tests/**",
    "**/__tests__/**",
    "**/*.test.*",
    "**/*.spec.*",
    "**/test.rs",
    "**/tests.rs",
    "**/*_test.rs",
    "**/*_test.go",
    "**/test_*.py",
    "**/snapshots/**",
    "**/__snapshots__/**",
    "**/fixtures/**",
];

/// The most patterns a team's list holds.
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
    /// `/` or `./`, or is not a glob pattern.
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
                    && !pattern.starts_with('/')
                    && !pattern.starts_with("./");
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

    /// Whether `path`, from the repository's root, is a Control File,
    /// whatever its case: a tool may read `Makefile` and `makefile` alike.
    pub fn matches(&self, path: &str) -> bool {
        let options = MatchOptions {
            case_sensitive: false,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        self.patterns
            .iter()
            .any(|pattern| pattern.matches_with(path, options))
    }

    /// The Control Files a branch that changed `changed` changed: those
    /// `request` asks for by naming them, when `request_asks` (its Run was
    /// created in person), and the others, which make it Tampered. None when
    /// it changed none; unlisted when GitHub could not list the changes
    /// (`None`), or either list is longer than a Run keeps.
    pub fn changes(
        &self,
        changed: Option<&[String]>,
        request: &str,
        request_asks: bool,
    ) -> Option<ControlFileChanges> {
        self.changes_guarding(changed, request, request_asks, None)
    }

    /// [`Self::changes`], with the Control Files `guarded` matches always
    /// Tampered, whatever the request names: a person approves each change
    /// to them before a merge.
    pub fn changes_guarding(
        &self,
        changed: Option<&[String]>,
        request: &str,
        request_asks: bool,
        guarded: Option<&ControlFiles>,
    ) -> Option<ControlFileChanges> {
        if self.is_empty() {
            return None;
        }
        let unlisted = ControlFileChanges {
            tampered: Vec::new(),
            named: Vec::new(),
            unlisted: true,
        };
        let Some(changed) = changed else {
            return Some(unlisted);
        };

        let mut tampered: Vec<String> = Vec::new();
        let mut named: Vec<String> = Vec::new();
        for path in changed {
            let seen = tampered.contains(path) || named.contains(path);
            if seen || !self.matches(path) {
                continue;
            }
            let always_tampered = guarded.is_some_and(|guarded| guarded.matches(path));
            if request_asks && !always_tampered && names(request, path) {
                named.push(path.clone());
            } else {
                tampered.push(path.clone());
            }
        }

        if tampered.is_empty() && named.is_empty() {
            return None;
        }
        // A rename lists both of its paths, so a comparison GitHub lists in
        // full can still name more files than a Run keeps.
        let max = ControlFileChanges::MAX_FILES;
        if tampered.len() > max || named.len() > max {
            return Some(unlisted);
        }
        Some(ControlFileChanges {
            tampered,
            named,
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
