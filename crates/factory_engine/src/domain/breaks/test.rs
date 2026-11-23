use factory_records::domain::breaks::BreakStatus;

use super::*;

const LINE: &str = "BREAK toggle_saves: crates/app/src/toggle.rs \"saved = true\" -> \"saved = false\" run `cargo test -p app toggle_saves`";

fn read(line: &str) -> Break {
    match read_line(line) {
        Some(Reported::Break(read)) => read,
        other => panic!("{line:?} reads as {other:?}"),
    }
}

fn why(line: &str) -> &'static str {
    match read_line(line) {
        Some(Reported::Unreadable(unreadable)) => unreadable.why,
        other => panic!("{line:?} reads as {other:?}"),
    }
}

#[test]
fn a_break_line_reads_as_its_test_file_texts_and_command() {
    assert_eq!(
        read(LINE),
        Break {
            test: "toggle_saves".to_string(),
            file: "crates/app/src/toggle.rs".to_string(),
            old_text: "saved = true".to_string(),
            new_text: "saved = false".to_string(),
            command: "cargo test -p app toggle_saves".to_string(),
        }
    );
}

/// A list mark, spaces in the test's name and quotes inside the texts all
/// read: the test runs to the first `: `, the old text to the first
/// `" -> "`, and the new text to the last `" run \``.
#[test]
fn list_marks_spaced_tests_and_quoted_texts_read() {
    let read = read(
        "  - BREAK app/src/calc.test.ts > double > doubles: app/src/calc.ts \"return \"x\" * 2\" -> \"return \"x\"\" run `bunx vitest run app/src/calc.test.ts -t doubles`",
    );

    assert_eq!(read.test, "app/src/calc.test.ts > double > doubles");
    assert_eq!(read.file, "app/src/calc.ts");
    assert_eq!(read.old_text, "return \"x\" * 2");
    assert_eq!(read.new_text, "return \"x\"");
    assert_eq!(
        read.command,
        "bunx vitest run app/src/calc.test.ts -t doubles"
    );
}

/// A break may delete its old text: the new text may be empty.
#[test]
fn a_break_may_delete_its_old_text() {
    let read = read("BREAK t: a.rs \"x += 1;\" -> \"\" run `cargo test t`");

    assert_eq!(read.new_text, "");
}

#[test]
fn lines_that_do_not_start_with_break_are_not_breaks() {
    for line in [
        "Wrote the toggle and its test.",
        "BREAKING: the toggle's API changed",
        "The BREAK line follows.",
        "",
    ] {
        assert_eq!(read_line(line), None, "{line:?}");
    }
}

#[test]
fn a_break_line_that_does_not_read_says_why() {
    let cases = [
        (
            "BREAK toggle_saves crates/app/src/toggle.rs \"a\" -> \"b\" run `t`",
            "no `: ` follows the test",
        ),
        ("BREAK : a.rs \"a\" -> \"b\" run `t`", "it names no test"),
        (
            "BREAK t: a.rs \"a\" -> \"b\"",
            "no run `<command>` follows the new text",
        ),
        (
            "BREAK t: a.rs \"a\" -> \"b\" run `t` and more",
            "the command does not end with a backtick",
        ),
        (
            "BREAK t: a.rs \"a\" -> \"b\" run `t `x``",
            "the command holds a backtick",
        ),
        (
            "BREAK t: a.rs \"a\" -> \"b\" run ` `",
            "the command is empty",
        ),
        (
            "BREAK t: a.rs \"a\" \"b\" run `t`",
            "no -> \"<new text>\" follows the old text",
        ),
        (
            "BREAK t: a.rs \"\" -> \"b\" run `t`",
            "the old text is empty",
        ),
        (
            "BREAK t: a.rs \"a\" -> \"a\" run `t`",
            "the new text is the old text",
        ),
        (
            "BREAK t: ../a.rs \"a\" -> \"b\" run `t`",
            "the file is not a path inside the repository",
        ),
        (
            "BREAK t: /etc/passwd \"a\" -> \"b\" run `t`",
            "the file is not a path inside the repository",
        ),
        (
            "BREAK t: .git/config \"a\" -> \"b\" run `t`",
            "the file is not a path inside the repository",
        ),
        (
            "BREAK t: src/./a.rs \"a\" -> \"b\" run `t`",
            "the file is not a path inside the repository",
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(why(line), expected, "{line:?}");
    }
    let long = format!("BREAK t: a.rs \"{}\" -> \"b\" run `t`", "a".repeat(1_025));
    assert_eq!(why(&long), "a part of it is longer than 1 KiB");
}

/// The Step's hand backs are read in order: a later line for a test
/// replaces the earlier one where it stood, and an unreadable line is kept
/// by its line.
#[test]
fn a_later_break_of_a_test_replaces_the_earlier() {
    let work = format!(
        "Wrote the toggle.\n{LINE}\nBREAK toggle_keeps: a.rs \"a\" -> \"b\" run `t`\nBREAK broken line"
    );
    let fix = "Fixed the break.\nBREAK toggle_saves: crates/app/src/toggle.rs \"saved\" -> \"lost\" run `cargo test -p app toggle_saves`";

    let breaks = reported([work.as_str(), fix]);

    let tests: Vec<&str> = breaks.iter().map(Reported::test).collect();
    assert_eq!(tests, ["toggle_saves", "toggle_keeps", "BREAK broken line"]);
    let Reported::Break(first) = &breaks[0] else {
        panic!("a break");
    };
    assert_eq!(first.old_text, "saved");
    assert!(matches!(breaks[2], Reported::Unreadable(_)));
}

#[test]
fn an_unreadable_line_is_recorded_unreadable_whatever_it_is_given() {
    let line = read_line("BREAK t: a.rs").expect("a break line");

    let record = line.record(BreakStatus::Verified, Some("abc"), "ignored");

    assert_eq!(record.status, BreakStatus::Unreadable);
    assert_eq!(record.test, "BREAK t: a.rs");
    assert_eq!(record.checked_commit, None);
    assert!(
        record
            .output
            .contains("no quoted old text follows the file")
    );
}

#[test]
fn a_record_keeps_the_end_of_a_long_output() {
    let long = format!("{}the end", "é".repeat(MAX_BREAK_OUTPUT_BYTES));

    let record = Reported::Break(read(LINE)).record(BreakStatus::NotRun, Some("abc"), &long);

    assert!(record.output.len() <= MAX_BREAK_OUTPUT_BYTES);
    assert!(record.output.ends_with("the end"));
}

fn run(stdout: &str) -> BreakRun {
    BreakRun::read(stdout)
}

/// The test passed without the break and failed with it: only then is the
/// break verified.
#[test]
fn a_break_run_is_verified_only_when_the_test_passed_then_failed() {
    let cases = [
        (
            "macro-break: unbroken 0\nmacro-break: broken 101\nmacro-break: restored\n",
            BreakStatus::Verified,
        ),
        (
            "macro-break: unbroken 0\nmacro-break: broken 124\n",
            BreakStatus::Verified,
        ),
        (
            "macro-break: unbroken 0\nmacro-break: broken 0\nmacro-break: restored\n",
            BreakStatus::NotReproduced,
        ),
        ("macro-break: unbroken 1\n", BreakStatus::NotRun),
        (
            "macro-break: unbroken 0\nmacro-break: not-applied: the file does not hold the old text\nmacro-break: restored\n",
            BreakStatus::NotApplied,
        ),
        (
            "macro-break: not-applied: no such file\n",
            BreakStatus::NotApplied,
        ),
        ("", BreakStatus::NotRun),
        ("macro-break: unbroken 0\n", BreakStatus::NotRun),
    ];
    for (stdout, status) in cases {
        assert_eq!(run(stdout).status(), status, "{stdout:?}");
    }
    assert!(run("macro-break: restored").restored);
    assert!(!run("macro-break: unbroken 0").restored);
}
