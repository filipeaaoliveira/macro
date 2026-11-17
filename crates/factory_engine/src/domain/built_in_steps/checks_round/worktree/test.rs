//! The worktree's shell, run for real by bash and git on this machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use super::*;

const ROUND: Uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);

/// A repository whose pushed commit says `pushed`, whose agent then
/// committed again and left work uncommitted, with a cache directory of its
/// own, in a directory of its own.
struct Machine {
    root: PathBuf,
    pushed: String,
    later: String,
}

impl Machine {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("macro-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let mut machine = Self {
            root,
            pushed: String::new(),
            later: String::new(),
        };
        machine.git(&["init", "--quiet"]);
        std::fs::write(machine.repo().join("a.txt"), "pushed\n").unwrap();
        machine.git(&["add", "a.txt"]);
        machine.git(&["commit", "--quiet", "-m", "pushed"]);
        machine.pushed = machine.head();
        std::fs::write(machine.repo().join("a.txt"), "committed, not pushed\n").unwrap();
        machine.git(&["commit", "--quiet", "-am", "not pushed"]);
        machine.later = machine.head();
        std::fs::write(machine.repo().join("a.txt"), "uncommitted\n").unwrap();
        machine.shim_timeout();
        machine
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    fn worktree(&self) -> PathBuf {
        self.cache().join("macro-check").join(ROUND.to_string())
    }

    fn head(&self) -> String {
        String::from_utf8(self.git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }

    fn git(&self, args: &[&str]) -> Output {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Factory",
                "-c",
                "user.email=factory@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(self.repo())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output
    }

    /// A machine without GNU `timeout` (this one, maybe) gets one that runs
    /// the command without a limit: the tests never reach a timeout.
    fn shim_timeout(&self) {
        if has("timeout") {
            return;
        }
        let shim = self.root.join("bin").join("timeout");
        std::fs::write(
            &shim,
            "#!/bin/bash\nwhile [[ $1 == --* ]]; do shift; done\nshift\nexec \"$@\"\n",
        )
        .unwrap();
        Command::new("chmod").arg("+x").arg(&shim).status().unwrap();
    }

    /// Run `script` with bash from the checkout, as the session runs it.
    fn bash(&self, script: &str) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var_os("PATH").unwrap().to_string_lossy()
        );
        self.bash_with_path(script, &path)
    }

    fn bash_with_path(&self, script: &str, path: &str) -> Output {
        Command::new("/bin/bash")
            .args(["--noprofile", "--norc", "-c", script])
            .current_dir(self.repo())
            .env("XDG_CACHE_HOME", self.cache())
            .env("PATH", path)
            .output()
            .unwrap()
    }

    /// A `PATH` of what a check's run needs, without `flock`.
    fn path_without_flock(&self) -> String {
        let tools = self.root.join("tools");
        std::fs::create_dir_all(&tools).unwrap();
        for tool in ["mkdir", "cat", "mv", "rm", "sleep", "bash", "timeout"] {
            let found = self.root.join("bin").join(tool);
            let target = if found.exists() {
                found
            } else {
                PathBuf::from(
                    String::from_utf8(
                        Command::new("bash")
                            .args(["-c", &format!("command -v {tool}")])
                            .output()
                            .unwrap()
                            .stdout,
                    )
                    .unwrap()
                    .trim(),
                )
            };
            let _ = std::os::unix::fs::symlink(target, tools.join(tool));
        }
        tools.display().to_string()
    }

    fn prepare(&self, commit: &str) -> Output {
        let output = self.bash(&prepare_command(ROUND, commit));
        assert!(output.status.success(), "{output:?}");
        assert!(prepared(&text(&output.stdout), commit));
        output
    }

    /// Hold the worktree's lock as a run still going would, for `seconds`,
    /// in a process of its own that nothing waits on: its pid.
    fn hold_lock(&self, seconds: u32) -> u32 {
        let lock = format!("{}.lock", self.worktree().display());
        if !has("flock") {
            return self.hold_dir_lock(seconds);
        }
        let holder = background(&format!("flock '{lock}' sleep {seconds}"));
        // The holder holds the lock once a non-blocking try fails.
        while Command::new("flock")
            .args(["-n", &lock, "true"])
            .status()
            .unwrap()
            .success()
        {}
        holder
    }

    /// Hold the worktree's `mkdir` lock with a live process, for `seconds`:
    /// its pid.
    fn hold_dir_lock(&self, seconds: u32) -> u32 {
        let holder = background(&format!("sleep {seconds}"));
        let held = PathBuf::from(format!("{}.lock.d", self.worktree().display()));
        std::fs::create_dir_all(&held).unwrap();
        std::fs::write(held.join("pid"), holder.to_string()).unwrap();
        holder
    }
}

/// Start `command` in the background of a shell that exits at once, so the
/// system, not this test, reaps it: its pid.
fn background(command: &str) -> u32 {
    let started = Command::new("bash")
        .args(["-c", &format!("{command} >/dev/null 2>&1 & echo $!")])
        .output()
        .unwrap();
    text(&started.stdout).trim().parse().unwrap()
}

fn stop(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).status();
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn has(tool: &str) -> bool {
    Command::new("bash")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .unwrap()
        .status
        .success()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The worktree is at the pushed commit before the checks run, a check
/// sees that commit and nothing the agent did since in its checkout, and
/// the worktree is gone once removed.
#[test]
fn checks_run_on_the_pushed_commit_whatever_the_checkout_holds() {
    let machine = Machine::new("pushed");

    machine.prepare(&machine.pushed);
    assert!(machine.worktree().is_dir());

    let checked = machine.bash(&check_command(
        ROUND,
        "cat a.txt; git status --porcelain",
        5,
        5,
    ));
    assert!(checked.status.success(), "{checked:?}");
    assert_eq!(text(&checked.stdout), "pushed\n");
    assert_eq!(
        std::fs::read_to_string(machine.repo().join("a.txt")).unwrap(),
        "uncommitted\n"
    );

    let removed = machine.bash(&remove_command(ROUND));
    assert!(removed.status.success(), "{removed:?}");
    assert!(!machine.worktree().exists());
}

/// A worktree an earlier attempt left at the commit is kept, and reset to
/// it: a change made there is gone.
#[test]
fn a_worktree_left_at_the_commit_is_reset_to_it() {
    let machine = Machine::new("reuse");
    machine.prepare(&machine.pushed);
    std::fs::write(machine.worktree().join("a.txt"), "changed by a check\n").unwrap();

    machine.prepare(&machine.pushed);

    assert_eq!(
        std::fs::read_to_string(machine.worktree().join("a.txt")).unwrap(),
        "pushed\n"
    );
}

/// A worktree left at another commit is replaced by one at the commit.
#[test]
fn a_worktree_left_at_another_commit_is_replaced() {
    let machine = Machine::new("stale-commit");
    machine.prepare(&machine.later);

    machine.prepare(&machine.pushed);

    assert_eq!(
        std::fs::read_to_string(machine.worktree().join("a.txt")).unwrap(),
        "pushed\n"
    );
}

/// Worktrees a round left behind over four hours ago go when another is
/// set up; newer ones stay.
#[test]
fn worktrees_left_behind_for_hours_are_removed() {
    let machine = Machine::new("leak");
    let base = machine.cache().join("macro-check");
    let old = base.join("old-round");
    let recent = base.join("recent-round");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&recent).unwrap();
    std::fs::File::open(&old)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(5 * 3600))
        .unwrap();

    machine.prepare(&machine.pushed);

    assert!(!old.exists());
    assert!(recent.exists());
}

/// A commit the repository lacks and cannot fetch sets no worktree up.
#[test]
fn a_commit_that_cannot_be_fetched_sets_nothing_up() {
    let machine = Machine::new("missing");
    let missing = "dddddddddddddddddddddddddddddddddddddddd";

    let failed = machine.bash(&prepare_command(ROUND, missing));

    assert_eq!(failed.status.code(), Some(1));
    assert!(!prepared(&text(&failed.stdout), missing));
    assert!(!machine.worktree().exists());
}

/// A run that finds the lock held for longer than its wait gives up saying
/// so, running nothing.
#[test]
fn a_run_that_finds_the_lock_held_gives_up() {
    let machine = Machine::new("locked");
    machine.prepare(&machine.pushed);
    let holder = machine.hold_lock(30);

    let busy = machine.bash(&check_command(ROUND, "echo ran", 5, 1));
    stop(holder);

    assert_eq!(busy.status.code(), Some(LOCK_BUSY_STATUS));
    assert!(text(&busy.stderr).contains(LOCK_BUSY));
    assert!(!text(&busy.stdout).contains("ran"));
}

/// Waiting for the lock does not count against the check's timeout: a
/// one-second check that waited two seconds for it still runs.
#[test]
fn waiting_for_the_lock_is_not_the_check_s_time() {
    let machine = Machine::new("wait");
    machine.prepare(&machine.pushed);
    machine.hold_lock(2);

    let ran = machine.bash(&check_command(ROUND, "echo ran", 1, 10));

    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(text(&ran.stdout), "ran\n");
}

/// Without `flock`, a directory lock whose holder died is taken over, and
/// the run says it locked with mkdir.
#[test]
fn a_directory_lock_whose_holder_died_is_taken() {
    let machine = Machine::new("stale-lock");
    machine.prepare(&machine.pushed);
    let dead = machine.hold_dir_lock(0);
    while Command::new("kill")
        .args(["-0", &dead.to_string()])
        .status()
        .unwrap()
        .success()
    {}

    let ran = machine.bash_with_path(
        &check_command(ROUND, "echo ran", 5, 1),
        &machine.path_without_flock(),
    );

    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(text(&ran.stdout), "ran\n");
    assert!(text(&ran.stderr).contains(NO_FLOCK));
    assert!(!Path::new(&format!("{}.lock.d", machine.worktree().display())).exists());
}
