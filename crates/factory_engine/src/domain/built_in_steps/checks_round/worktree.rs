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
//! the repository. Beside the worktree, outside the repository's tree, a
//! file lists the paths the change touched, which each check is told of
//! with the change's base ([`crate::domain::check_context`]).

#[cfg(test)]
mod test;

use std::time::Duration;

use uuid::Uuid;

use crate::domain::breaks::{Break, MARKER};
pub(super) use crate::domain::check_context::is_commit;
use crate::domain::check_context::{BASE_VARIABLE, CHANGED_VARIABLE, CheckContext, Listing};
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
/// (reset to it), and replace one at another commit. Then, for the checks,
/// fetch `context`'s base the same way when the repository lacks it (a
/// fetch that fails is said and changes nothing else), and write the file
/// listing the changed paths beside the worktree, outside the repository's
/// tree ([`changed_file_command`]).
pub(super) fn prepare_command(round: Uuid, commit: &str, context: &CheckContext) -> String {
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
check_base={check_base}
base_missing=
if [ -n \"$check_base\" ] && ! g cat-file -e \"$check_base^{{commit}}\" 2>/dev/null; then
  if [ \"$(g rev-parse --is-shallow-repository)\" = true ]; then
    ( commit=$check_base; fetch_commit --depth=1 )
  else
    ( commit=$check_base; fetch_commit )
  fi || {{ echo \"macro-check: could not fetch the base $check_base\" >&2; base_missing=1; }}
fi
{changed}
echo \"{READY} $commit\"",
        commit = shell_quote(commit),
        check_base = shell_quote(&context.base),
        changed = changed_file_command(context),
    )
}

/// Write `context`'s list of changed paths to the file beside the
/// worktree (`$worktree.changed`), through a file of its own moved into
/// place, so a check never reads half of it. A `complete` list whose base
/// could not be fetched says `partial` instead, so no check diffs against
/// a commit the repository lacks.
fn changed_file_command(context: &CheckContext) -> String {
    let paths: String = context
        .paths
        .iter()
        .map(|path| format!(" {}", shell_quote(path)))
        .collect();
    format!(
        "listing={listing}
if [ -n \"$base_missing\" ] && [ \"$listing\" = {complete} ]; then listing={partial}; fi
printf '%s\\n' \"$listing\"{paths} > \"$worktree.changed.new\" && mv -f \"$worktree.changed.new\" \"$worktree.changed\" || {{ echo 'macro-check: the list of changed paths could not be written' >&2; exit 1; }}",
        listing = context.listing.as_str(),
        complete = Listing::Complete.as_str(),
        partial = Listing::Partial.as_str(),
    )
}

/// What a check is told of the change, `context`, as shell that exports
/// it in the check command's own shell, before the check and the
/// repository's setup `env` run: [`BASE_VARIABLE`], and
/// [`CHANGED_VARIABLE`] naming the file [`prepare_command`] wrote.
fn context_exports(context: &CheckContext) -> String {
    format!(
        "export {BASE_VARIABLE}={} {CHANGED_VARIABLE}=\"$worktree.changed\"\n",
        shell_quote(&context.base)
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
/// lock, so nothing it leaves running holds it. A check, told the change's
/// `context`, runs with it exported ([`context_exports`]); a setup command,
/// told none, without.
pub(super) fn check_command(
    round: Uuid,
    run: &str,
    timeout_secs: u64,
    lock_wait_secs: u64,
    context: Option<&CheckContext>,
) -> String {
    format!(
        "{BASE}worktree=\"$base/{round}\"
lock=\"$worktree.lock\"
{take_lock}cd \"$worktree\" || exit 1
{exports}timeout --kill-after=10 {timeout_secs} bash --noprofile --norc -c {run} 9>&-",
        run = shell_quote(run),
        take_lock = take_lock(lock_wait_secs),
        exports = context.map(context_exports).unwrap_or_default(),
    )
}

/// The most of a break's test run's output its file holds, in KiB (bash's
/// `ulimit -f` blocks).
pub(super) const BREAK_OUT_KIB: u64 = 64 * 1024;

/// How much of the end of each of a break's test runs is shown, in bytes:
/// both runs' and the markers stay well under the 16 KiB a runtime that
/// keeps one merged stream returns.
pub(super) const BREAK_TAIL_BYTES: usize = 1_500;

/// Run one break, `reported`, in the round `round`'s worktree, holding the
/// worktree's lock as a check does: first put every tracked file back as
/// the commit has it (a break an earlier run left is undone), then run the
/// test's command for at most `timeout_secs`, after `environment` (what
/// the round's checks run after: the repository's setup `env`), and told
/// the change's `context` as a check is; when it passed, replace the first
/// old text in the break's file with the new text, run the command again,
/// and put the file back. The file is rewritten rather than copied back
/// with its old time, so a build tool that goes by times builds it again.
///
/// Each run is a process group of its own (`set -m`), watched by a shell
/// watchdog of its own group: past `timeout_secs` it marks the run hung,
/// sends the whole group `TERM`, and `KILL` ten seconds later, so a test
/// stops on time whatever children it started, and the run ends with
/// [`crate::domain::breaks::HUNG_STATUS`]. Once the test's shell returns,
/// on time or not, its group and the watchdog's are killed, so nothing the
/// test left running outlives it or prints after it. Its output goes to a
/// file outside the worktree's tree, held to [`BREAK_OUT_KIB`] (`ulimit -f`,
/// a write past it failing rather than stopping the test), and only the
/// last [`BREAK_TAIL_BYTES`] are printed, on the standard error. No
/// `timeout` is used, so nothing depends on its flags. The markers [`crate::domain::breaks::BreakRun`] reads
/// are printed last, on the standard output, each with `nonce`, so a
/// runtime that merges both streams and keeps only their end still returns
/// them, and nothing the test prints reads as one. A run stopped before its
/// end prints none.
///
/// A file that is not a regular file in the worktree, or whose path goes
/// through a symbolic link (checked again just before the edit, as the
/// test's first run may have changed the tree), or that lacks the old text,
/// is not broken; a
/// read break's new text always differs from its old, so a replacement
/// changes the file. The edit needs only POSIX `awk`, which reads the texts
/// from its environment, as they are.
pub(super) fn break_command(
    round: Uuid,
    reported: &Break,
    environment: &str,
    nonce: &str,
    timeout_secs: u64,
    lock_wait_secs: u64,
    context: &CheckContext,
) -> String {
    format!(
        "{GIT}{BASE}worktree=\"$base/{round}\"
lock=\"$worktree.lock\"
{take_lock}cd \"$worktree\" || exit 1
{exports}g reset --hard -q HEAD || exit 1
file={file}
saved=\"$worktree.break\"
out=\"$worktree.break.out\"
marks=
mark() {{ marks=\"${{marks}}{MARKER} {nonce}: $1
\"; }}
finish() {{ printf '%s' \"$marks\"; exit 0; }}
run_test() {{
  rm -f \"$out\" \"$out.hung\"
  set -m
  (
    trap '' XFSZ
    ulimit -f {out_blocks}
    exec bash --noprofile --norc -c {command} 9>&- >\"$out\" 2>&1 </dev/null
  ) &
  group=$!
  (
    sleep {timeout_secs}
    : > \"$out.hung\"
    kill -TERM -- \"-$group\" 2>/dev/null
    sleep 10
    kill -KILL -- \"-$group\" 2>/dev/null
  ) 9>&- >/dev/null 2>&1 </dev/null &
  watchdog=$!
  set +m
  wait \"$group\"
  ran=$?
  kill -KILL -- \"-$group\" 2>/dev/null
  kill -KILL -- \"-$watchdog\" 2>/dev/null
  wait \"$watchdog\" 2>/dev/null
  if [ -e \"$out.hung\" ]; then ran={hung}; fi
  echo \"--- the test $1 exited with status $ran; the end of its output:\" >&2
  tail -c {tail} \"$out\" >&2
  echo >&2
  rm -f \"$out\" \"$out.hung\"
  return $ran
}}
in_tree() {{
  rest=$file
  at=
  while [ -n \"$rest\" ]; do
    part=${{rest%%/*}}
    if [ \"$part\" = \"$rest\" ]; then rest=; else rest=${{rest#*/}}; fi
    at=${{at:+$at/}}$part
    [ -L \"$at\" ] && return 1
  done
  [ -f \"$file\" ]
}}
in_tree || {{ mark 'not-applied: no such file'; finish; }}
run_test 'without the break'
unbroken=$?
mark \"unbroken $unbroken\"
[ \"$unbroken\" -eq 0 ] || finish
in_tree || {{ mark 'not-applied: no such file'; finish; }}
cat \"$file\" > \"$saved\" || exit 1
BREAK_OLD={old} BREAK_NEW={new} awk '
  !done {{ at = index($0, ENVIRON[\"BREAK_OLD\"]) }}
  !done && at > 0 {{ $0 = substr($0, 1, at - 1) ENVIRON[\"BREAK_NEW\"] substr($0, at + length(ENVIRON[\"BREAK_OLD\"])); done = 1 }}
  {{ print }}
  END {{ exit done ? 0 : 3 }}
' \"$saved\" > \"$file\"
applied=$?
if [ \"$applied\" -eq 0 ]; then
  run_test 'with the break'
  mark \"broken $?\"
else
  mark 'not-applied: the file does not hold the old text'
fi
cat \"$saved\" > \"$file\" && rm -f \"$saved\" && mark restored
finish",
        file = shell_quote(&reported.file),
        command = shell_quote(&format!("{environment}{}", reported.command)),
        old = shell_quote(&reported.old_text),
        new = shell_quote(&reported.new_text),
        nonce = nonce,
        tail = BREAK_TAIL_BYTES,
        out_blocks = BREAK_OUT_KIB,
        hung = crate::domain::breaks::HUNG_STATUS,
        take_lock = take_lock(lock_wait_secs),
        exports = context_exports(context),
    )
}

/// Remove the round `round`'s worktree, its lock and its list of changed
/// paths, from the checkout the command runs in.
pub(super) fn remove_command(round: Uuid) -> String {
    format!(
        "{GIT}{BASE}worktree=\"$base/{round}\"
g worktree remove --force \"$worktree\" 2>/dev/null || rm -rf \"$worktree\"
g worktree prune
rm -rf \"$worktree.lock\" \"$worktree.lock.d\" \"$worktree.changed\" \"$worktree.changed.new\" \"$worktree.break\" \"$worktree.break.out\""
    )
}

/// `text` as one single-quoted shell word.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}
