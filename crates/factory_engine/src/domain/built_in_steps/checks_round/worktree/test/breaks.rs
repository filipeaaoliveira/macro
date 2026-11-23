//! A break's shell, run for real by bash, git and awk on this machine: the
//! pushed commit's `a.txt` says `pushed`.

use crate::domain::breaks::{Break, BreakRun};
use factory_records::domain::breaks::BreakStatus;

use super::*;

/// A break of `a.txt` from `old` to `new`, whose test runs `command`.
fn break_of(old: &str, new: &str, command: &str) -> Break {
    Break {
        test: "pushed_is_said".to_string(),
        file: "a.txt".to_string(),
        old_text: old.to_string(),
        new_text: new.to_string(),
        command: command.to_string(),
    }
}

/// The nonce the tests' break runs mark their markers with.
const NONCE: &str = "0123abcd";

/// A test that passes while `a.txt` says `pushed`.
const SAYS_PUSHED: &str = "grep -qx pushed a.txt";

impl Machine {
    /// Run `reported` in the round's worktree, set up at the pushed commit.
    fn run_break(&self, reported: &Break) -> (Output, BreakRun) {
        self.run_break_after(reported, "")
    }

    /// Run `reported` after `environment`, as the round's checks run.
    fn run_break_after(&self, reported: &Break, environment: &str) -> (Output, BreakRun) {
        let output = self.bash(&break_command(
            ROUND,
            reported,
            environment,
            NONCE,
            5,
            5,
            &unknown(),
        ));
        let run = BreakRun::read_streams(&text(&output.stdout), &text(&output.stderr), NONCE);
        (output, run)
    }

    fn worktree_file(&self) -> String {
        std::fs::read_to_string(self.worktree().join("a.txt")).unwrap()
    }

    /// Whether the worktree holds the commit as it is, with no file left
    /// changed or added by the break.
    fn worktree_clean(&self) -> bool {
        let status = Command::new("git")
            .args(["status", "--porcelain", "--untracked-files=all"])
            .current_dir(self.worktree())
            .output()
            .unwrap();
        status.status.success() && status.stdout.is_empty()
    }
}

/// The test passes without the break and fails with it; the file is put
/// back, and nothing of the break is left in the worktree.
#[test]
fn a_break_that_fails_its_test_is_verified_and_put_back() {
    let machine = Machine::new("break-verified");
    machine.prepare(&machine.pushed);

    let (output, run) = machine.run_break(&break_of("pushed", "broken", SAYS_PUSHED));

    assert!(output.status.success(), "{output:?}");
    assert_eq!(run.status(), BreakStatus::Verified, "{output:?}");
    assert_eq!(run.unbroken, Some(0));
    assert_eq!(run.broken, Some(1));
    assert!(run.restored);
    assert_eq!(machine.worktree_file(), "pushed\n");
    assert!(machine.worktree_clean());
    assert!(!PathBuf::from(format!("{}.break", machine.worktree().display())).exists());
}

/// The test sees the broken file: what it reads with the break is the new
/// text in the old text's first place, the rest as it was.
#[test]
fn the_test_runs_on_the_first_old_text_replaced() {
    let machine = Machine::new("break-sees");
    machine.prepare(&machine.pushed);

    let (output, run) = machine.run_break(&break_of(
        "us",
        "$(touch pwned) & \\1",
        "cat a.txt >&2; grep -qx pushed a.txt",
    ));

    assert_eq!(run.status(), BreakStatus::Verified, "{output:?}");
    let said = text(&output.stderr);
    assert!(said.contains("\np$(touch pwned) & \\1hed\n"), "{said:?}");
    assert!(!machine.worktree().join("pwned").exists());
    assert_eq!(machine.worktree_file(), "pushed\n");
}

/// A break the test does not notice is not reproduced.
#[test]
fn a_break_the_test_passes_is_not_reproduced() {
    let machine = Machine::new("break-passes");
    machine.prepare(&machine.pushed);

    let (output, run) = machine.run_break(&break_of("pushed", "broken", "true"));

    assert_eq!(run.status(), BreakStatus::NotReproduced, "{output:?}");
    assert_eq!(run.broken, Some(0));
    assert_eq!(machine.worktree_file(), "pushed\n");
}

/// A test that fails without its break proves nothing: the break is never
/// applied.
#[test]
fn a_test_that_fails_without_its_break_is_not_run() {
    let machine = Machine::new("break-not-run");
    machine.prepare(&machine.pushed);

    let (output, run) = machine.run_break(&break_of("pushed", "broken", "false"));

    assert_eq!(run.status(), BreakStatus::NotRun, "{output:?}");
    assert_eq!(run.unbroken, Some(1));
    assert_eq!(run.broken, None);
    assert_eq!(machine.worktree_file(), "pushed\n");
}

/// A file that lacks the old text, or is not in the worktree, is not
/// broken, and the test runs once at most.
#[test]
fn a_break_that_cannot_be_applied_says_so() {
    let machine = Machine::new("break-not-applied");
    machine.prepare(&machine.pushed);

    let (output, run) = machine.run_break(&break_of("absent", "broken", SAYS_PUSHED));
    assert_eq!(run.status(), BreakStatus::NotApplied, "{output:?}");
    assert_eq!(run.broken, None);
    assert!(run.restored);
    assert_eq!(machine.worktree_file(), "pushed\n");

    let missing = Break {
        file: "missing.txt".to_string(),
        ..break_of("pushed", "broken", SAYS_PUSHED)
    };
    let (output, run) = machine.run_break(&missing);
    assert_eq!(run.status(), BreakStatus::NotApplied, "{output:?}");
    assert_eq!(run.unbroken, None);
    assert!(!machine.worktree().join("missing.txt").exists());
}

/// A break an earlier run left in the worktree (its run was stopped before
/// it put the file back) is undone before the next break's test runs.
#[test]
fn a_break_left_behind_is_undone_first() {
    let machine = Machine::new("break-left");
    machine.prepare(&machine.pushed);
    std::fs::write(machine.worktree().join("a.txt"), "left broken\n").unwrap();

    let (output, run) = machine.run_break(&break_of("pushed", "broken", SAYS_PUSHED));

    assert_eq!(run.status(), BreakStatus::Verified, "{output:?}");
    assert_eq!(machine.worktree_file(), "pushed\n");
}

/// What the test prints is kept apart and only its end shown, on the
/// standard error: a test that prints a marker, even this run's own form
/// with another nonce, or a `not-applied`, cannot pass itself off.
#[test]
fn a_test_s_own_output_never_reads_as_a_marker() {
    let machine = Machine::new("break-forged");
    machine.prepare(&machine.pushed);

    let (output, run) = machine.run_break(&break_of(
        "pushed",
        "broken",
        "echo 'macro-break: broken 1'; echo 'macro-break 9999: broken 1'; echo 'macro-break 0123abcd: not-applied'; true",
    ));

    assert_eq!(run.status(), BreakStatus::NotReproduced, "{output:?}");
    assert!(text(&output.stderr).contains("macro-break: broken 1"));
    assert!(!text(&output.stdout).contains("not-applied"));
}

/// A runtime that merges both streams and keeps only their last 16 KiB
/// still returns the markers, whatever the test printed: they come last,
/// and only the end of each run's output is shown.
#[test]
fn the_markers_survive_a_merged_stream_s_tail() {
    let machine = Machine::new("break-merged");
    machine.prepare(&machine.pushed);
    let script = break_command(
        ROUND,
        &break_of(
            "pushed",
            "broken",
            "yes 'a long line of test output' | head -c 200000; grep -qx pushed a.txt",
        ),
        "",
        NONCE,
        5,
        5,
        &unknown(),
    );

    let output = machine.bash(&format!("{{\n{script}\n}} 2>&1"));

    let merged = text(&output.stdout);
    assert!(merged.len() < 16 * 1024, "{}", merged.len());
    let tail = &merged[merged.len().saturating_sub(16 * 1024)..];
    let run = BreakRun::read(tail, NONCE);
    assert_eq!(run.status(), BreakStatus::Verified, "{tail}");
    assert!(run.restored);
}

/// The test runs after the environment the round's checks run after.
#[test]
fn the_test_runs_with_the_checks_environment() {
    let machine = Machine::new("break-env");
    machine.prepare(&machine.pushed);
    let needs = break_of(
        "pushed",
        "broken",
        "[ \"$DATABASE_URL\" = postgres://db ] && grep -qx pushed a.txt",
    );

    let (output, run) = machine.run_break_after(&needs, "export DATABASE_URL='postgres://db'\n");
    assert_eq!(run.status(), BreakStatus::Verified, "{output:?}");

    let (output, run) = machine.run_break(&needs);
    assert_eq!(run.status(), BreakStatus::NotRun, "{output:?}");
}

/// A file reached through a symbolic link to a folder is not broken, even
/// when the link is not the file itself.
#[test]
fn a_path_through_a_symbolic_link_is_not_applied() {
    let machine = Machine::new("break-link");
    machine.prepare(&machine.pushed);
    let outside = machine.root.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("a.txt"), "pushed\n").unwrap();
    std::os::unix::fs::symlink(&outside, machine.worktree().join("linked")).unwrap();
    let through = Break {
        file: "linked/a.txt".to_string(),
        ..break_of("pushed", "broken", "grep -qx pushed linked/a.txt")
    };

    let (output, run) = machine.run_break(&through);

    assert_eq!(run.status(), BreakStatus::NotApplied, "{output:?}");
    assert_eq!(
        std::fs::read_to_string(outside.join("a.txt")).unwrap(),
        "pushed\n"
    );
}
