//! The lock a command run on the agent's machine holds, so a retry's run
//! never works alongside a run it replaces that is still going (a worker
//! lost mid-run leaves its command running): with `flock` where the
//! machine has it, else with a directory, which a run that died does not
//! hold. A checks round's checks and setup hold their worktree's; a
//! session's setup holds one in its checkout.

/// How long a run waits for the lock before it gives the run up to a
/// retry, in seconds: on top of the run's own timeout.
pub const LOCK_WAIT_SECS: u64 = 60;

/// The exit status a run takes when another run holds the lock.
pub const LOCK_BUSY_STATUS: i32 = 75;

/// What a run says when another run holds the lock.
pub const LOCK_BUSY: &str = "macro: another run holds this lock";

/// What a run says when the machine has no `flock`. Both messages go in
/// single quotes in the shell, so neither has an apostrophe.
pub const NO_FLOCK: &str = "macro: this machine has no flock; the run locks with mkdir";

/// The shell that takes the lock at `$lock`, waiting at most
/// `lock_wait_secs`, else exits with [`LOCK_BUSY_STATUS`] saying
/// [`LOCK_BUSY`]. The lock is held on descriptor 9 (or by `$lock.d`) until
/// the shell exits; the command that follows closes 9 (`9>&-`), so nothing
/// it leaves running holds the lock.
pub fn take_lock(lock_wait_secs: u64) -> String {
    format!(
        "if command -v flock >/dev/null 2>&1; then
  exec 9>\"$lock\" && flock -w {lock_wait_secs} 9 || {{ echo '{LOCK_BUSY}' >&2; exit {LOCK_BUSY_STATUS}; }}
else
  echo '{NO_FLOCK}' >&2
  waited=0
  until mkdir \"$lock.d\" 2>/dev/null; do
    holder=$(cat \"$lock.d/pid\" 2>/dev/null)
    if [ -n \"$holder\" ] && ! kill -0 \"$holder\" 2>/dev/null; then
      stale=\"$lock.d.stale.$$\"
      mv \"$lock.d\" \"$stale\" 2>/dev/null && rm -rf \"$stale\"
      continue
    fi
    waited=$((waited + 1))
    if [ \"$waited\" -ge {lock_wait_secs} ]; then echo '{LOCK_BUSY}' >&2; exit {LOCK_BUSY_STATUS}; fi
    sleep 1
  done
  echo $$ > \"$lock.d/pid\"
  trap 'rm -rf \"$lock.d\"' EXIT
fi
"
    )
}

/// Whether a run's output says another run held the lock.
pub fn lock_busy(exit_code: i32, stdout: &str, stderr: &str) -> bool {
    exit_code == LOCK_BUSY_STATUS && (stderr.contains(LOCK_BUSY) || stdout.contains(LOCK_BUSY))
}
