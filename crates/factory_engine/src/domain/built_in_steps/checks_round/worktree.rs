//! The shell a verified round runs on the agent's machine: a worktree of
//! the agent's repository, detached at the pushed commit, that the round's
//! checks run in and that is removed after them.
//!
//! The checks never run in the agent's own checkout, so nothing the agent
//! left there (uncommitted work, a commit it did not push, a changed git
//! hook) is what they check. Git runs from an absolute path where the
//! machine has one, without system or global configuration (a fetch alone
//! keeps the global one, for its credentials), hooks, a filesystem monitor,
//! replace objects or a sparse checkout, and the worktree is checked to
//! hold the commit as it is, with no entry git is told to skip. Worktrees
//! live in a directory of the machine user's own, `0700`, where those a
//! round left behind for four hours are removed. Each check holds the
//! worktree's lock for its run, waiting a bounded time for a run still
//! going (a retry's) before its own timeout starts, never around a read of
//! the repository.

#[cfg(test)]
mod test;

use std::time::Duration;

use uuid::Uuid;

use crate::domain::shell_lock::take_lock;

pub(super) use crate::domain::shell_lock::{LOCK_BUSY, LOCK_BUSY_STATUS, LOCK_WAIT_SECS, NO_FLOCK};

/// What a check's session command may take beyond the check's own timeout:
/// the lock's wait, `timeout`'s ten seconds before it kills, and a margin.
pub(super) const RUN_MARGIN: Duration = Duration::from_secs(LOCK_WAIT_SECS + 20);

/// What setting the worktree up says once it is at the commit.
const READY: &str = "macro-check: worktree at";

/// How long setting the worktree up may take: a fetch at worst.
pub(super) const PREPARE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long removing the worktree may take.
pub(super) const REMOVE_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Where a round's worktree lives, in the shell: the machine user's own
/// cache, outside the agent's checkout, named by the round.
const BASE: &str = "base=\"${XDG_CACHE_HOME:-$HOME/.cache}/macro-check\"\n";

/// Git, found once by absolute path where the machine has one (else on its
/// `PATH`): `g` reads no system or global configuration and runs no hook,
/// filesystem monitor, replace object or sparse checkout; `fetch_commit`
/// keeps the global configuration, where the machine's credentials are,
/// and runs no hook either.
const GIT: &str = "export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null GIT_TERMINAL_PROMPT=0 GIT_NO_REPLACE_OBJECTS=1
git_bin=
for candidate in /usr/bin/git /run/current-system/sw/bin/git /nix/var/nix/profiles/default/bin/git; do
  if [ -x \"$candidate\" ]; then git_bin=$candidate; break; fi
done
if [ -z \"$git_bin\" ]; then git_bin=$(command -v git) || { echo 'macro-check: git is missing' >&2; exit 1; }; fi
g() { \"$git_bin\" -c core.fsmonitor= -c core.hooksPath=/dev/null -c core.sparseCheckout=false -c extensions.worktreeConfig=false -c 'safe.directory=*' \"$@\"; }
fetch_commit() { env -u GIT_CONFIG_GLOBAL \"$git_bin\" -c core.fsmonitor= -c core.hooksPath=/dev/null -c 'safe.directory=*' fetch --quiet \"$@\" origin \"$commit\"; }
";

/// Set the round `round`'s worktree up at `commit`, from the checkout the
/// command runs in: remove worktrees left behind for four hours, fetch the
/// commit when the repository lacks it (only its own depth in a shallow
/// clone), keep a worktree an earlier attempt left at the same commit
/// (reset to it), and replace one at another commit.
pub(super) fn prepare_command(round: Uuid, commit: &str) -> String {
    format!(
        "{GIT}{BASE}commit={commit}
worktree=\"$base/{round}\"
mkdir -p \"$base\" && chmod 700 \"$base\" || exit 1
find \"$base\" -mindepth 1 -maxdepth 1 -mmin +240 | while IFS= read -r old; do
  g worktree remove --force \"$old\" 2>/dev/null || rm -rf \"$old\"
done
g worktree prune
if ! g cat-file -e \"$commit^{{commit}}\" 2>/dev/null; then
  if [ \"$(g rev-parse --is-shallow-repository)\" = true ]; then
    fetch_commit --depth=1 || {{ echo \"macro-check: could not fetch $commit\" >&2; exit 1; }}
  else
    fetch_commit || {{ echo \"macro-check: could not fetch $commit\" >&2; exit 1; }}
  fi
fi
if [ -d \"$worktree\" ] && [ \"$(g -C \"$worktree\" rev-parse HEAD 2>/dev/null)\" = \"$commit\" ]; then
  g -C \"$worktree\" reset --hard -q \"$commit\" || exit 1
elif [ -e \"$worktree\" ]; then
  g worktree remove --force \"$worktree\" 2>/dev/null || rm -rf \"$worktree\"
  g worktree prune
fi
if [ ! -d \"$worktree\" ]; then
  g worktree add --quiet --detach \"$worktree\" \"$commit\" || exit 1
fi
[ \"$(g -C \"$worktree\" rev-parse HEAD)\" = \"$commit\" ] || {{ echo \"macro-check: the worktree is not at $commit\" >&2; exit 1; }}
g -C \"$worktree\" diff --quiet \"$commit\" -- || {{ echo \"macro-check: the worktree differs from $commit\" >&2; exit 1; }}
if g -C \"$worktree\" ls-files -v | grep -qE '^([a-z]|S) '; then
  echo 'macro-check: the worktree has entries git is told to skip' >&2
  exit 1
fi
echo \"{READY} $commit\"",
        commit = shell_quote(commit),
    )
}

/// Whether a set-up's output says the worktree is at `commit`.
pub(super) fn prepared(stdout: &str, commit: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.trim() == format!("{READY} {commit}"))
}

/// Run one check, `run`, in the round `round`'s worktree for at most
/// `timeout_secs`, holding the worktree's lock: with `flock` where the
/// machine has it, else with a directory, which a run that died does not
/// hold. The lock is waited for at most `lock_wait_secs` before the check's
/// timeout starts; a run that waits that long gives up with
/// [`LOCK_BUSY_STATUS`] and [`LOCK_BUSY`]. The check does not inherit the
/// lock, so nothing it leaves running holds it.
pub(super) fn check_command(
    round: Uuid,
    run: &str,
    timeout_secs: u64,
    lock_wait_secs: u64,
) -> String {
    format!(
        "{BASE}worktree=\"$base/{round}\"
lock=\"$worktree.lock\"
{take_lock}cd \"$worktree\" || exit 1
timeout --kill-after=10 {timeout_secs} bash --noprofile --norc -c {run} 9>&-",
        run = shell_quote(run),
        take_lock = take_lock(lock_wait_secs),
    )
}

/// Remove the round `round`'s worktree, and its lock, from the checkout the
/// command runs in.
pub(super) fn remove_command(round: Uuid) -> String {
    format!(
        "{GIT}{BASE}worktree=\"$base/{round}\"
g worktree remove --force \"$worktree\" 2>/dev/null || rm -rf \"$worktree\"
g worktree prune
rm -rf \"$worktree.lock\" \"$worktree.lock.d\""
    )
}

/// Whether `commit` names a commit as GitHub gives it: 40 or 64 lowercase
/// hex digits.
pub(super) fn is_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
