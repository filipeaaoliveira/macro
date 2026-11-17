//! One command run in a session's checkout, where its agent works: the
//! repository a sandbox or a Macro computer cloned for it.
//!
//! The harness runs it beside the agent, never through it, so its exit code
//! is the command's own. Runtimes the harness cannot reach into (a paired
//! machine, a cloud agent, the in-process agent) answer
//! [`AgentSessionError::CommandUnsupported`](crate::domain::error::AgentSessionError::CommandUnsupported).

use std::time::Duration;

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod test;

/// The longest a command may run.
pub const MAX_COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The most of each of a command's streams kept: its last bytes, where a
/// failure usually says why.
pub const MAX_COMMAND_OUTPUT_BYTES: usize = 16 * 1024;

/// What ends a stream cut to [`MAX_COMMAND_OUTPUT_BYTES`], ahead of it.
const CUT_MARK: &str = "(earlier output left out)\n";

/// A command to run in a session's checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCommand {
    /// The directory it runs in, relative to the checkout's root: `.` for
    /// the root itself. Never absolute, and never above the root.
    pub cwd: String,
    /// A shell command line, run with `bash`.
    pub command: String,
    /// How long it may run, in seconds, at most [`MAX_COMMAND_TIMEOUT`];
    /// past it the command is stopped and reported as timed out.
    pub timeout_secs: u64,
}

/// Why a [`SessionCommand`] is not one the harness runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSessionCommand {
    /// The command line is blank.
    #[error("the command is empty")]
    EmptyCommand,
    /// The directory is absolute or leaves the checkout.
    #[error("the directory must stay inside the checkout")]
    OutsideCheckout,
    /// The timeout is zero or past [`MAX_COMMAND_TIMEOUT`].
    #[error("the timeout must be between 1 second and 30 minutes")]
    Timeout,
}

impl SessionCommand {
    /// The command's timeout.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    /// Check that the command can run: a command line, a directory inside
    /// the checkout and a bounded timeout.
    pub fn validate(&self) -> Result<(), InvalidSessionCommand> {
        if self.command.trim().is_empty() {
            return Err(InvalidSessionCommand::EmptyCommand);
        }
        let leaves = self.cwd.starts_with('/')
            || self.cwd.split('/').any(|part| part == "..")
            || self.cwd.contains('\0');
        if leaves {
            return Err(InvalidSessionCommand::OutsideCheckout);
        }
        if self.timeout_secs == 0 || self.timeout() > MAX_COMMAND_TIMEOUT {
            return Err(InvalidSessionCommand::Timeout);
        }
        Ok(())
    }

    /// The shell script that runs it from `checkout`, the checkout's
    /// absolute path, under its timeout: `timeout` stops it with `TERM`,
    /// then `KILL` ten seconds later, and exits 124 (or 137).
    ///
    /// The command runs in a shell that reads no profile or rc file, which
    /// the agent could have changed. It holds a lock in the checkout's git
    /// directory while it runs (`flock`, where the machine has it), so a
    /// retried run of the same check waits for one still running, within
    /// the same timeout.
    ///
    /// Each part is quoted for the shell, so the script is safe to hand to
    /// `bash -c` as it is.
    pub fn script(&self, checkout: &str) -> String {
        let directory = if self.cwd.is_empty() || self.cwd == "." {
            checkout.to_owned()
        } else {
            format!("{}/{}", checkout.trim_end_matches('/'), self.cwd)
        };
        let run = format!("bash --noprofile --norc -c {}", shell_quote(&self.command));
        let timeout = format!("timeout --kill-after=10 {}", self.timeout_secs);
        format!(
            "cd {} && git_dir=$(git rev-parse --absolute-git-dir 2>/dev/null); \
             if [ -n \"$git_dir\" ] && command -v flock >/dev/null 2>&1; \
             then exec {timeout} flock \"$git_dir/{LOCK_FILE}\" {run}; fi; \
             exec {timeout} {run}",
            shell_quote(&directory),
        )
    }
}

/// The lock a command holds in its checkout's git directory while it runs.
const LOCK_FILE: &str = "macro-check.lock";

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionCommandOutput {
    /// Its exit code: 0 when it succeeded, 124 or 137 when its timeout
    /// stopped it.
    pub exit_code: i32,
    /// The last [`MAX_COMMAND_OUTPUT_BYTES`] of its standard output. A
    /// runtime that keeps the two streams together puts both here.
    pub stdout: String,
    /// The last [`MAX_COMMAND_OUTPUT_BYTES`] of its standard error; empty
    /// where the runtime keeps the streams together.
    pub stderr: String,
}

impl SessionCommandOutput {
    /// An output from what the command wrote, each stream cut to its last
    /// [`MAX_COMMAND_OUTPUT_BYTES`].
    pub fn new(exit_code: i32, stdout: &[u8], stderr: &[u8]) -> Self {
        Self {
            exit_code,
            stdout: tail(stdout),
            stderr: tail(stderr),
        }
    }
}

/// The last [`MAX_COMMAND_OUTPUT_BYTES`] of `bytes` as text, marked when
/// cut, and never split inside a character.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_COMMAND_OUTPUT_BYTES {
        return text.into_owned();
    }
    let mut start = text.len() - (MAX_COMMAND_OUTPUT_BYTES - CUT_MARK.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{CUT_MARK}{}", &text[start..])
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
