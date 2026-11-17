//! The worktree's shell, run for real by bash and git on this machine.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};

use super::*;

/// A repository whose pushed commit says `pushed`, whose agent then
/// committed again and left work uncommitted, in a directory of its own.
struct Machine {
    root: PathBuf,
    pushed: String,
}

impl Machine {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("macro-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("repo")).unwrap();
        let mut machine = Self {
            root,
            pushed: String::new(),
        };
        machine.git(&["init", "--quiet"]);
        std::fs::write(machine.repo().join("a.txt"), "pushed\n").unwrap();
        machine.git(&["add", "a.txt"]);
        machine.git(&["commit", "--quiet", "-m", "pushed"]);
        let pushed = String::from_utf8(machine.git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_owned();
        std::fs::write(machine.repo().join("a.txt"), "committed, not pushed\n").unwrap();
        machine.git(&["commit", "--quiet", "-am", "not pushed"]);
        std::fs::write(machine.repo().join("a.txt"), "uncommitted\n").unwrap();
        machine.pushed = pushed;
        machine
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn worktree(&self) -> String {
        self.root.join("worktree").to_str().unwrap().to_owned()
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

    /// Run `script` with bash from the checkout, as the session runs it.
    fn bash(&self, script: &str) -> Output {
        Command::new("bash")
            .args(["--noprofile", "--norc", "-c", script])
            .current_dir(self.repo())
            .output()
            .unwrap()
    }

    /// Hold the worktree's lock as a run still going would, until the
    /// child is killed.
    fn hold_lock(&self) -> Child {
        let lock = format!("{}.lock", self.worktree());
        let has_flock = Command::new("bash")
            .args(["-c", "command -v flock"])
            .output()
            .unwrap()
            .status
            .success();
        if has_flock {
            let child = Command::new("flock")
                .args([&lock, "sleep", "30"])
                .spawn()
                .unwrap();
            // The child holds the lock once a non-blocking try fails.
            while Command::new("flock")
                .args(["-n", &lock, "true"])
                .status()
                .unwrap()
                .success()
            {}
            child
        } else {
            let child = Command::new("sleep").arg("30").spawn().unwrap();
            let held = PathBuf::from(format!("{lock}.d"));
            std::fs::create_dir_all(&held).unwrap();
            std::fs::write(held.join("pid"), child.id().to_string()).unwrap();
            child
        }
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
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
    let worktree = machine.worktree();

    let prepared_run = machine.bash(&prepare_command(&worktree, &machine.pushed));
    assert!(prepared_run.status.success(), "{prepared_run:?}");
    assert!(prepared(&text(&prepared_run.stdout), &machine.pushed));
    assert!(Path::new(&worktree).is_dir());

    let checked = machine.bash(&check_command(
        &worktree,
        "cat a.txt; git status --porcelain",
        5,
    ));
    assert!(checked.status.success(), "{checked:?}");
    assert_eq!(text(&checked.stdout), "pushed\n");
    assert_eq!(
        std::fs::read_to_string(machine.repo().join("a.txt")).unwrap(),
        "uncommitted\n"
    );

    let removed = machine.bash(&remove_command(&worktree));
    assert!(removed.status.success(), "{removed:?}");
    assert!(!Path::new(&worktree).exists());
}

/// A commit the repository lacks and cannot fetch sets no worktree up.
#[test]
fn a_commit_that_cannot_be_fetched_sets_nothing_up() {
    let machine = Machine::new("missing");
    let worktree = machine.worktree();

    let failed = machine.bash(&prepare_command(
        &worktree,
        "dddddddddddddddddddddddddddddddddddddddd",
    ));

    assert_eq!(failed.status.code(), Some(1));
    assert!(!prepared(
        &text(&failed.stdout),
        "dddddddddddddddddddddddddddddddddddddddd"
    ));
    assert!(!Path::new(&worktree).exists());
}

/// A run that finds the lock held waits its bounded time, then gives up
/// saying so, running nothing.
#[test]
fn a_run_that_finds_the_lock_held_gives_up() {
    let machine = Machine::new("locked");
    let worktree = machine.worktree();
    let prepared_run = machine.bash(&prepare_command(&worktree, &machine.pushed));
    assert!(prepared_run.status.success(), "{prepared_run:?}");
    let mut holder = machine.hold_lock();

    let busy = machine.bash(&check_command(&worktree, "echo ran", 1));
    holder.kill().unwrap();
    holder.wait().unwrap();

    assert_eq!(busy.status.code(), Some(LOCK_BUSY_STATUS));
    assert!(text(&busy.stderr).contains(LOCK_BUSY));
    assert!(!text(&busy.stdout).contains("ran"));
}

/// Where the lock was a directory and its holder died, the next run takes
/// it.
#[test]
fn a_lock_whose_holder_died_is_taken() {
    let machine = Machine::new("stale");
    let worktree = machine.worktree();
    let prepared_run = machine.bash(&prepare_command(&worktree, &machine.pushed));
    assert!(prepared_run.status.success(), "{prepared_run:?}");
    let mut dead = Command::new("true").spawn().unwrap();
    dead.wait().unwrap();
    let held = PathBuf::from(format!("{worktree}.lock.d"));
    std::fs::create_dir_all(&held).unwrap();
    std::fs::write(held.join("pid"), dead.id().to_string()).unwrap();

    let ran = machine.bash(&check_command(&worktree, "echo ran", 1));

    assert!(ran.status.success(), "{ran:?}");
    assert_eq!(text(&ran.stdout), "ran\n");
}
