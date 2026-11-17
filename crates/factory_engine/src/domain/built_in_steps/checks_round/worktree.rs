//! The shell a verified round runs on the agent's machine: a worktree of
//! the agent's repository, detached at the pushed commit, that the round's
//! checks run in and that is removed after them.
//!
//! The checks never run in the agent's own checkout, so nothing the agent
//! left there (uncommitted work, a commit it did not push, a changed git
//! hook) is what they check. Git runs from an absolute path where the
//! machine has one, without system or global configuration, hooks or a
//! filesystem monitor. Each check holds a lock on the worktree for its run,
//! waiting a bounded time for a run still going (a retry's), never around a
//! read of the repository.

#[cfg(test)]
mod test;

use std::time::Duration;

use uuid::Uuid;

/// How long a check waits for the worktree's lock before it gives the run
/// up to a retry, in seconds.
pub(super) const LOCK_WAIT_SECS: u64 = 60;

/// The exit status a check's run takes when another run holds the lock.
pub(super) const LOCK_BUSY_STATUS: i32 = 75;

/// What a check's run says when another run holds the lock.
pub(super) const LOCK_BUSY: &str = "macro-check: another run holds the lock on this worktree";

/// What a check's run says when the machine has no `flock`. Both messages
/// go in single quotes in the shell, so neither has an apostrophe.
pub(super) const NO_FLOCK: &str =
    "macro-check: this machine has no flock; the run locks with mkdir";

/// What setting the worktree up says once it is at the commit.
const READY: &str = "macro-check: worktree at";

/// How long setting the worktree up may take: a fetch at worst.
pub(super) const PREPARE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long removing the worktree may take.
pub(super) const REMOVE_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Where a round's worktree lives on the agent's machine: outside its
/// checkout, named by the round.
pub(super) fn worktree_path(check_action_id: Uuid) -> String {
    format!("/tmp/macro-check-{check_action_id}")
}

/// Git, found once by absolute path where the machine has one (else on its
/// `PATH`), as the `g` function: no system or global configuration, no
/// hooks, no filesystem monitor.
const GIT: &str = "export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0
git_bin=
for candidate in /usr/bin/git /run/current-system/sw/bin/git /nix/var/nix/profiles/default/bin/git; do
  if [ -x \"$candidate\" ]; then git_bin=$candidate; break; fi
done
if [ -z \"$git_bin\" ]; then git_bin=$(command -v git) || { echo 'macro-check: git is missing' >&2; exit 1; }; fi
g() { \"$git_bin\" -c core.fsmonitor= -c core.hooksPath=/dev/null \"$@\"; }
";

/// Set the worktree at `path` up at `commit`, from the checkout the command
/// runs in: fetch the commit when the repository lacks it, and keep a
/// worktree an earlier attempt left at the same commit.
pub(super) fn prepare_command(path: &str, commit: &str) -> String {
    format!(
        "{GIT}commit={commit}
worktree={path}
if ! g cat-file -e \"$commit^{{commit}}\" 2>/dev/null; then
  g fetch --quiet origin \"$commit\" || {{ echo \"macro-check: could not fetch $commit\" >&2; exit 1; }}
fi
if [ -d \"$worktree\" ] && [ \"$(g -C \"$worktree\" rev-parse HEAD 2>/dev/null)\" != \"$commit\" ]; then
  g worktree remove --force \"$worktree\" 2>/dev/null || rm -rf \"$worktree\"
fi
if [ ! -d \"$worktree\" ]; then
  g worktree prune
  g worktree add --quiet --detach \"$worktree\" \"$commit\" || exit 1
fi
[ \"$(g -C \"$worktree\" rev-parse HEAD)\" = \"$commit\" ] || {{ echo \"macro-check: the worktree is not at $commit\" >&2; exit 1; }}
echo \"{READY} $commit\"",
        path = shell_quote(path),
    )
}

/// Whether a set-up's output says the worktree is at `commit`.
pub(super) fn prepared(stdout: &str, commit: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.trim() == format!("{READY} {commit}"))
}

/// Run one check, `run`, in the worktree at `path`, holding its lock: with
/// `flock` where the machine has it, else with a directory, which a run
/// that died does not hold. A run that waits `lock_wait_secs` for another
/// gives up with [`LOCK_BUSY_STATUS`] and [`LOCK_BUSY`].
pub(super) fn check_command(path: &str, run: &str, lock_wait_secs: u64) -> String {
    format!(
        "worktree={path}
lock=\"$worktree.lock\"
if command -v flock >/dev/null 2>&1; then
  exec 9>\"$lock\" && flock -w {lock_wait_secs} 9 || {{ echo '{LOCK_BUSY}' >&2; exit {LOCK_BUSY_STATUS}; }}
else
  echo '{NO_FLOCK}' >&2
  waited=0
  until mkdir \"$lock.d\" 2>/dev/null; do
    holder=$(cat \"$lock.d/pid\" 2>/dev/null)
    if [ -n \"$holder\" ] && ! kill -0 \"$holder\" 2>/dev/null; then rm -rf \"$lock.d\"; continue; fi
    waited=$((waited + 1))
    if [ \"$waited\" -ge {lock_wait_secs} ]; then echo '{LOCK_BUSY}' >&2; exit {LOCK_BUSY_STATUS}; fi
    sleep 1
  done
  echo $$ > \"$lock.d/pid\"
  trap 'rm -rf \"$lock.d\"' EXIT
fi
cd \"$worktree\" || exit 1
{run}",
        path = shell_quote(path),
    )
}

/// Remove the worktree at `path`, and its lock, from the checkout the
/// command runs in.
pub(super) fn remove_command(path: &str) -> String {
    format!(
        "{GIT}worktree={path}
g worktree remove --force \"$worktree\" 2>/dev/null || rm -rf \"$worktree\"
g worktree prune
rm -rf \"$worktree.lock\" \"$worktree.lock.d\"",
        path = shell_quote(path),
    )
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
