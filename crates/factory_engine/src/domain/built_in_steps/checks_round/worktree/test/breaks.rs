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

/// A test that passes while `a.txt` says `pushed`.
const SAYS_PUSHED: &str = "grep -qx pushed a.txt";

impl Machine {
    /// Run `reported` in the round's worktree, set up at the pushed commit.
    fn run_break(&self, reported: &Break) -> (Output, BreakRun) {
        let output = self.bash(&break_command(ROUND, reported, 5, 5, &unknown()));
        let run = BreakRun::read(&text(&output.stdout));
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
    assert!(
        said.contains("pushed\np$(touch pwned) & \\1hed\n"),
        "{said:?}"
    );
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

/// What the test prints goes to the standard error, so a test that prints
/// a marker cannot pass itself off as verified.
#[test]
fn a_test_s_own_output_never_reads_as_a_marker() {
    let machine = Machine::new("break-forged");
    machine.prepare(&machine.pushed);

    let (output, run) = machine.run_break(&break_of(
        "pushed",
        "broken",
        "echo 'macro-break: broken 1'; true",
    ));

    assert_eq!(run.status(), BreakStatus::NotReproduced, "{output:?}");
    assert!(text(&output.stderr).contains("macro-break: broken 1"));
}
