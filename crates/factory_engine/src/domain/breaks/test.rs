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
        BREAK_FORM,
        "    `BREAK <test>: <file> \"<old text>\" -> \"<new text>\" run `<command>``",
    ] {
        assert_eq!(read_line(line), None, "{line:?}");
    }
}

#[test]
fn a_break_line_that_does_not_read_says_why() {
    let cases = [
        (
            "BREAK toggle_saves crates/app/src/toggle.rs \"a\" -> \"b\" run `t`",
            "no `: <file> \"<old text>\"` follows the test",
        ),
        ("BREAK : a.rs \"a\" -> \"b\" run `t`", "it names no test"),
        (
            "BREAK t: .GIT/hooks/pre-commit \"a\" -> \"b\" run `t`",
            "the file is not a path inside the repository",
        ),
        (
            "BREAK t: a.rs \"a\" -> \"b\" -> \"c\" run `t`",
            "the texts hold `\" -> \"` more than once",
        ),
        (
            "BREAK t: a.rs \"a\" -> \"b\"",
            "no run `<command>` follows the quoted new text",
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
            .contains("no run `<command>` follows the quoted new text")
    );
}

#[test]
fn a_record_keeps_the_end_of_a_long_output() {
    let long = format!("{}the end", "é".repeat(MAX_BREAK_OUTPUT_BYTES));

    let record = Reported::Break(read(LINE)).record(BreakStatus::NotRun, Some("abc"), &long);

    assert!(record.output.len() <= MAX_BREAK_OUTPUT_BYTES);
    assert!(record.output.ends_with("the end"));
}

fn run(output: &str) -> BreakRun {
    BreakRun::read(output, "n1")
}

/// The test passed without the break and failed with it: only then is the
/// break verified.
#[test]
fn a_break_run_is_verified_only_when_the_test_passed_then_failed() {
    let cases = [
        (
            "macro-break n1: unbroken 0\nmacro-break n1: broken 101\nmacro-break n1: restored\n",
            BreakStatus::Verified,
        ),
        (
            "macro-break n1: unbroken 0\nmacro-break n1: broken 124\n",
            BreakStatus::Hung,
        ),
        (
            "macro-break n1: unbroken 0\nmacro-break n1: broken 0\nmacro-break n1: restored\n",
            BreakStatus::NotReproduced,
        ),
        ("macro-break n1: unbroken 1\n", BreakStatus::NotRun),
        (
            "macro-break n1: unbroken 0\nmacro-break n1: not-applied: the file does not hold the old text\nmacro-break n1: restored\n",
            BreakStatus::NotApplied,
        ),
        (
            "macro-break n1: not-applied: no such file\n",
            BreakStatus::NotApplied,
        ),
        ("", BreakStatus::Interrupted),
        ("macro-break n1: unbroken 0\n", BreakStatus::Interrupted),
        // Another run's markers, or ones with no nonce, are not this run's.
        (
            "macro-break n2: unbroken 0\nmacro-break n2: broken 1\nmacro-break: broken 1\n",
            BreakStatus::Interrupted,
        ),
    ];
    for (stdout, status) in cases {
        assert_eq!(run(stdout).status(), status, "{stdout:?}");
    }
    assert!(run("macro-break n1: restored").restored);
    assert!(!run("macro-break n1: unbroken 0").restored);
}

/// A test id may hold `: `, as a pytest parameter does: the test runs to
/// the `: ` that a file and a quoted old text follow.
#[test]
fn a_test_id_may_hold_a_colon_and_a_space() {
    let read = read(
        "BREAK tests/test_calc.py::test_double[a: 2]: calc.py \"return x * 2\" -> \"return x\" run `pytest 'tests/test_calc.py::test_double[a: 2]'`",
    );

    assert_eq!(read.test, "tests/test_calc.py::test_double[a: 2]");
    assert_eq!(read.file, "calc.py");
    assert_eq!(read.old_text, "return x * 2");
}

/// A Step keeps at most [`MAX_REPORTED`] tests' lines.
#[test]
fn at_most_fifty_tests_are_kept() {
    let work: String = (0..60)
        .map(|at| format!("BREAK t{at}: a.rs \"a\" -> \"b\" run `t`\n"))
        .collect();

    let breaks = reported([work.as_str()]);

    assert_eq!(breaks.len(), MAX_REPORTED);
    assert_eq!(breaks.last().map(Reported::test), Some("t49"));
}

/// Only the markers that end the output count: a marker of this run's
/// form before other lines, as a test that guessed the nonce would print
/// in a merged stream, is not one.
#[test]
fn only_the_markers_that_end_the_output_count() {
    let output = "macro-break n1: not-applied\n--- the test with the break exited with status 0\nmacro-break n1: unbroken 0\nmacro-break n1: broken 0\n";

    assert_eq!(run(output).status(), BreakStatus::NotReproduced);
    // Markers after other output, as a merged stream returns them, count.
    assert_eq!(
        run("noise\nmacro-break n1: unbroken 0\nmacro-break n1: broken 1\n").status(),
        BreakStatus::Verified
    );
}

/// A test that passed without its break and ran out of its time with it
/// hung: that proves nothing, so it is not verified.
#[test]
fn a_break_whose_test_hangs_is_hung_not_verified() {
    for code in [124, 137] {
        let output = format!("macro-break n1: unbroken 0\nmacro-break n1: broken {code}\n");
        assert_eq!(run(&output).status(), BreakStatus::Hung, "{code}");
    }
    assert_eq!(
        run("macro-break n1: unbroken 0\nmacro-break n1: broken 101\n").status(),
        BreakStatus::Verified
    );
}

/// Unreadable lines take room only while there is some: readable breaks
/// count toward the most a Step keeps, and push out the latest unreadable
/// ones past it.
#[test]
fn readable_breaks_count_toward_the_most_kept() {
    let mut work: String = (0..10).map(|at| format!("BREAK u{at}: no\n")).collect();
    work.extend((0..45).map(|at| format!("BREAK t{at}: a.rs \"a\" -> \"b\" run `t`\n")));

    let breaks = reported([work.as_str()]);

    assert_eq!(breaks.len(), MAX_REPORTED);
    let unreadable = breaks
        .iter()
        .filter(|kept| matches!(kept, Reported::Unreadable(_)))
        .count();
    assert_eq!(unreadable, 5);
    assert_eq!(breaks[4].test(), "BREAK u4: no");

    let mut only: String = (0..60).map(|at| format!("BREAK u{at}: no\n")).collect();
    only.extend((0..50).map(|at| format!("BREAK t{at}: a.rs \"a\" -> \"b\" run `t`\n")));
    let breaks = reported([only.as_str()]);
    assert_eq!(breaks.len(), MAX_REPORTED);
    assert!(breaks.iter().all(|kept| matches!(kept, Reported::Break(_))));
}
