//! How a repository prepares a fresh checkout before the factory works in
//! it: `.macro/setup.json` at its root, read from the Run's base branch as
//! the checks file is ([`crate::domain::checks`]).
//!
//! ```json
//! {
//!   "setup": [
//!     { "name": "install", "run": "bun install --frozen-lockfile" },
//!     { "name": "generate", "run": "just generate", "timeout": 300 }
//!   ],
//!   "env": { "CARGO_TARGET_DIR": "/tmp/target" }
//! }
//! ```
//!
//! Each command runs `run` from the repository's root, in order, with
//! `bash --noprofile --norc`, under its own `timeout` (seconds, 1 to 1200,
//! 600 when left out): before the first prompt of each new agent session of
//! a Step, and in a checks round's worktree before its checks. `env` holds
//! plain values every command, and every check, runs with, exported inside
//! the script `bash` runs. A value is taken as it is written, never
//! expanded: `"$HOME/cache"` is that text, not a path under the home. Secrets never
//! live here: a file holding something shaped like a key or a token, or an
//! `env` name that says it holds one, does not read.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};

use agent_session::domain::session_command::SessionCommandOutput;
use serde::Deserialize;

use crate::domain::checks::{DEFAULT_CHECK_TIMEOUT_SECS, MAX_CHECK_TIMEOUT_SECS, kept_output};
use crate::domain::definition::{UniqueKeys, secret_in, secret_in_value};
use crate::domain::shell_lock::take_lock;

/// Where a repository declares its setup, from its root.
pub const SETUP_FILE: &str = ".macro/setup.json";

/// The largest setup file read, in bytes.
const MAX_FILE_BYTES: usize = 16 * 1024;

/// The most commands a setup runs.
const MAX_COMMANDS: usize = 16;

/// The longest command a setup runs, in bytes.
const MAX_RUN_BYTES: usize = 4096;

/// The most values `env` holds.
const MAX_ENV: usize = 32;

/// The longest value in `env`, in bytes.
const MAX_ENV_VALUE_BYTES: usize = 1024;

/// The words of an `env` name that says it holds a secret, in any case.
/// Each is one or more whole words of the name, between `_`, so
/// `TOKENIZERS_PARALLELISM` and `GOOGLE_APPLICATION_CREDENTIALS` (a path)
/// are plain values.
const SECRET_NAMES: [&[&str]; 9] = [
    &["TOKEN"],
    &["SECRET"],
    &["PASSWORD"],
    &["PASSWD"],
    &["APIKEY"],
    &["API", "KEY"],
    &["PRIVATE", "KEY"],
    &["ACCESS", "KEY"],
    &["CREDENTIAL"],
];

/// The `env` names a setup may not set: those that change what
/// `bash --noprofile --norc` runs or traces, where the home is, and where
/// git finds the repository. `PATH` is allowed: the values are exported
/// inside the script, so it never reaches `timeout` or `bash`.
const RESERVED_NAMES: [&str; 11] = [
    "BASH_ENV",
    "ENV",
    "SHELLOPTS",
    "BASHOPTS",
    "PS4",
    "BASH_XTRACEFD",
    "HOME",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_CEILING_DIRECTORIES",
];

/// The beginnings of the names a setup may not set: the factory's own
/// variables, the dynamic linker's, which load code into every program,
/// and git's configuration.
const RESERVED_PREFIXES: [&str; 4] = ["MACRO_", "LD_", "DYLD_", "GIT_CONFIG"];

/// One setup command: a command run from the repository's root.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupCommand {
    /// Its name, unique among the setup's commands.
    pub name: String,
    /// The command, run from the repository's root.
    pub run: String,
    /// How many seconds it may take.
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    DEFAULT_CHECK_TIMEOUT_SECS
}

/// A repository's setup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositorySetup {
    /// The commands, in the order they run.
    #[serde(default)]
    pub setup: Vec<SetupCommand>,
    /// The plain values the commands, and the checks, run with.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// Why a repository's setup file does not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupFileError {
    /// The file is not the setup's JSON.
    #[error("{SETUP_FILE} does not read: {0}")]
    Unreadable(String),
    /// The file is larger than the factory reads.
    #[error("{SETUP_FILE} is over {MAX_FILE_BYTES} bytes")]
    TooLarge,
    /// The file holds something shaped like a secret.
    #[error(
        "{SETUP_FILE} holds what looks like a secret ({0}…): keep secrets out of the repository's setup"
    )]
    Secret(&'static str),
    /// It declares more commands than a setup runs.
    #[error("{SETUP_FILE} declares {0} commands; a setup runs at most {MAX_COMMANDS}")]
    TooMany(usize),
    /// A command's name is empty, too long, or not of `[a-z0-9_-]`.
    #[error("{SETUP_FILE} names a command {0:?}; a name is 1 to 64 of a-z, 0-9, _ and -")]
    Name(String),
    /// Two commands share a name.
    #[error("{SETUP_FILE} names two commands {0}")]
    DuplicateName(String),
    /// A command is blank or too long.
    #[error("{SETUP_FILE} gives command {0} a blank run or one over {MAX_RUN_BYTES} bytes")]
    Run(String),
    /// A command's timeout is zero or past [`MAX_CHECK_TIMEOUT_SECS`].
    #[error(
        "{SETUP_FILE} gives command {0} a timeout outside 1 to {MAX_CHECK_TIMEOUT_SECS} seconds"
    )]
    Timeout(String),
    /// `env` holds more values than a setup takes.
    #[error("{SETUP_FILE} sets {0} env values; a setup sets at most {MAX_ENV}")]
    TooManyEnv(usize),
    /// An `env` name is not a shell variable's, or is one the factory keeps.
    #[error(
        "{SETUP_FILE} sets env {0:?}; a name is a shell variable's (A-Z, a-z, 0-9 and _, not first a digit), and not one that changes the shell, the home, git or the linker, or the factory's own (MACRO_)"
    )]
    EnvName(String),
    /// An `env` name says it holds a secret.
    #[error("{SETUP_FILE} sets env {0}, whose name says it holds a secret: env holds plain values")]
    SecretEnv(String),
    /// An `env` value is too long, or holds a control character.
    #[error(
        "{SETUP_FILE} gives env {0} a value over {MAX_ENV_VALUE_BYTES} bytes or with a control character"
    )]
    EnvValue(String),
}

impl RepositorySetup {
    /// Read the setup from the text of the repository's setup file.
    pub fn parse(text: &str) -> Result<Self, SetupFileError> {
        if text.len() > MAX_FILE_BYTES {
            return Err(SetupFileError::TooLarge);
        }
        if let Some(kind) = secret_in(text) {
            return Err(SetupFileError::Secret(kind));
        }
        let unreadable = |error: serde_json::Error| SetupFileError::Unreadable(error.to_string());
        serde_json::from_str::<UniqueKeys>(text).map_err(unreadable)?;
        let value: serde_json::Value = serde_json::from_str(text).map_err(unreadable)?;
        // JSON escapes (`_`) hide a token from the text: its strings
        // are checked again once decoded.
        if let Some(kind) = secret_in_value(&value) {
            return Err(SetupFileError::Secret(kind));
        }
        let setup: Self = serde_json::from_value(value).map_err(unreadable)?;

        if setup.setup.len() > MAX_COMMANDS {
            return Err(SetupFileError::TooMany(setup.setup.len()));
        }
        let mut names = BTreeSet::new();
        for command in &setup.setup {
            let name_valid = (1..=64).contains(&command.name.len())
                && command.name.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-')
                });
            if !name_valid {
                return Err(SetupFileError::Name(command.name.clone()));
            }
            if !names.insert(command.name.as_str()) {
                return Err(SetupFileError::DuplicateName(command.name.clone()));
            }
            if command.run.trim().is_empty() || command.run.len() > MAX_RUN_BYTES {
                return Err(SetupFileError::Run(command.name.clone()));
            }
            if command.timeout == 0 || command.timeout > MAX_CHECK_TIMEOUT_SECS {
                return Err(SetupFileError::Timeout(command.name.clone()));
            }
        }

        if setup.env.len() > MAX_ENV {
            return Err(SetupFileError::TooManyEnv(setup.env.len()));
        }
        for (name, value) in &setup.env {
            if !env_name(name) {
                return Err(SetupFileError::EnvName(name.clone()));
            }
            let upper = name.to_ascii_uppercase();
            let words: Vec<&str> = upper.split('_').collect();
            if SECRET_NAMES
                .iter()
                .any(|secret| words.windows(secret.len()).any(|at| at == *secret))
            {
                return Err(SetupFileError::SecretEnv(name.clone()));
            }
            if value.len() > MAX_ENV_VALUE_BYTES || value.chars().any(char::is_control) {
                return Err(SetupFileError::EnvValue(name.clone()));
            }
        }
        Ok(setup)
    }

    /// Whether it asks for nothing: no command and no value.
    pub fn is_empty(&self) -> bool {
        self.setup.is_empty() && self.env.is_empty()
    }

    /// Its `env` as shell that exports each value, one line each; nothing
    /// when it holds none. A value is single-quoted: never expanded, so
    /// `$PATH` in one stays those five characters.
    pub fn exports(&self) -> String {
        self.env
            .iter()
            .map(|(name, value)| format!("export {name}={}\n", shell_quote(value)))
            .collect()
    }

    /// The shell that runs `command` from the directory it runs in, with
    /// the setup's `env`, under the command's timeout. The values are
    /// exported inside the script `bash` runs, so none reaches `timeout`
    /// or `bash` themselves.
    pub fn shell(&self, command: &SetupCommand) -> String {
        format!(
            "timeout --kill-after=10 {} bash --noprofile --norc -c {}",
            command.timeout,
            shell_quote(&format!("{}{}", self.exports(), command.run))
        )
    }

    /// [`Self::shell`] run from the root of a session's checkout, holding
    /// the checkout's setup lock ([`crate::domain::shell_lock`]), waited for
    /// at most `lock_wait_secs` before the command's timeout starts: a
    /// retry's command never runs alongside one an attempt it replaces left
    /// running.
    pub fn locked_shell(&self, command: &SetupCommand, lock_wait_secs: u64) -> String {
        format!(
            "{SESSION_LOCK}{}{} 9>&-",
            take_lock(lock_wait_secs),
            self.shell(command)
        )
    }
}

/// Where a session's setup keeps its lock, in the shell: in the checkout's
/// git directory, where nothing the agent commits reaches it, else at the
/// checkout's root.
const SESSION_LOCK: &str =
    "lock=$(git rev-parse --git-path macro-setup.lock 2>/dev/null) || lock=.macro-setup.lock\n";

/// What stands in a setup's output for a line that looks like a secret.
pub const REDACTED: &str = "(a line that looks like a secret is left out)";

/// The end of what a setup command printed, both streams, as a failure
/// shows it: each line that looks like a secret left out, then as a
/// verified round keeps a check's output.
pub fn output_tail(output: &SessionCommandOutput) -> String {
    let mut text = output.stdout.clone();
    if !output.stderr.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&output.stderr);
    }
    kept_output(&redacted(&text))
}

/// `text` with each line that looks like a secret (the Definition's check)
/// replaced by [`REDACTED`].
pub fn redacted(text: &str) -> String {
    text.split('\n')
        .map(|line| if secret_in(line).is_some() { REDACTED } else { line })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `name` is a shell variable's name the setup may set.
fn env_name(name: &str) -> bool {
    let mut chars = name.chars();
    let starts = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_');
    starts
        && name.len() <= 128
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !RESERVED_NAMES.contains(&name.to_ascii_uppercase().as_str())
        && !RESERVED_PREFIXES
            .iter()
            .any(|prefix| name.to_ascii_uppercase().starts_with(prefix))
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
