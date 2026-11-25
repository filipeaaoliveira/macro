use super::*;
use crate::diff::Hunk;

const LIB: &str = "crates/a/src/lib.rs";

fn entry(file: &str, function: &str, line: u32, cyclomatic: f64, crap: f64) -> Entry {
    Entry {
        file: file.to_owned(),
        function: function.to_owned(),
        line,
        cyclomatic,
        coverage: Some(0.0),
        crap,
    }
}

fn hunk(base_start: u32, base_count: u32, start: u32, count: u32) -> Hunk {
    Hunk {
        base_start,
        base_count,
        start,
        count,
    }
}

fn function(last: u32, body: &str) -> Function {
    Function {
        last,
        body: body.to_owned(),
    }
}

fn pass() -> Verdict {
    Verdict {
        threshold: 30.0,
        tests_failed: false,
    }
}

/// `crates/a/src/lib.rs`, a line changed at 5 and at 30, with functions at
/// 1-10, 12-20, 22-40 and 42-50.
fn touched_lib() -> TouchedFile {
    TouchedFile {
        diff: FileDiff {
            path: LIB.to_owned(),
            base_path: Some(LIB.to_owned()),
            hunks: vec![hunk(5, 1, 5, 1), hunk(30, 1, 30, 1)],
        },
        functions: BTreeMap::from([
            (1, function(10, "{ 1 }")),
            (12, function(20, "{ 12 }")),
            (22, function(40, "{ 22 }")),
            (42, function(50, "{ 42 }")),
        ]),
    }
}

fn summary(rows: &[Row]) -> Vec<(&str, Change, Option<f64>, bool)> {
    rows.iter()
        .map(|row| {
            (
                row.name.as_str(),
                row.change,
                row.base_complexity,
                row.fails,
            )
        })
        .collect()
}

#[test]
fn only_touched_functions_get_rows() {
    let current = [
        entry(LIB, "touched", 1, 2.0, 2.0),
        entry(LIB, "untouched", 12, 9.0, 90.0),
        entry("crates/b/src/lib.rs", "elsewhere", 1, 9.0, 90.0),
    ];
    let base = [entry(LIB, "touched", 1, 2.0, 0.0)];

    let rows = rows(&current, &base, &[touched_lib()], &[], pass());

    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["touched"]);
}

#[test]
fn a_new_or_more_complex_function_above_the_threshold_fails() {
    let current = [
        entry(LIB, "added", 1, 6.0, 42.0),
        entry(LIB, "grown", 22, 7.0, 56.0),
    ];
    let base = [entry(LIB, "grown", 22, 6.0, 0.0)];

    let rows = rows(&current, &base, &[touched_lib()], &[], pass());

    assert_eq!(
        summary(&rows),
        [
            ("added", Change::New, None, true),
            ("grown", Change::Rose, Some(6.0), true),
        ]
    );
}

#[test]
fn a_touched_function_no_more_complex_than_at_the_base_is_reported_without_failing() {
    let current = [
        entry(LIB, "same", 1, 6.0, 42.0),
        entry(LIB, "simpler", 22, 6.0, 42.0),
    ];
    let base = [
        entry(LIB, "same", 1, 6.0, 0.0),
        entry(LIB, "simpler", 22, 8.0, 0.0),
    ];

    let rows = rows(&current, &base, &[touched_lib()], &[], pass());

    assert_eq!(
        summary(&rows),
        [
            ("same", Change::Same, Some(6.0), false),
            ("simpler", Change::Fell, Some(8.0), false),
        ]
    );
}

#[test]
fn a_new_function_at_or_below_the_threshold_passes() {
    let current = [entry(LIB, "small", 1, 5.0, 30.0)];

    let rows = rows(&current, &[], &[touched_lib()], &[], pass());

    assert_eq!(summary(&rows), [("small", Change::New, None, false)]);
}

#[test]
fn a_main_function_gets_no_row() {
    let current = [entry(LIB, "main", 1, 9.0, 90.0)];

    assert!(rows(&current, &[], &[touched_lib()], &[], pass()).is_empty());
}

#[test]
fn a_renamed_file_pairs_with_its_functions_under_the_earlier_name() {
    let mut file = touched_lib();
    file.diff.base_path = Some("crates/a/src/old.rs".to_owned());
    let current = [entry(LIB, "moved", 1, 9.0, 90.0)];
    let base = [
        entry(LIB, "moved", 1, 1.0, 0.0),
        entry("crates/a/src/old.rs", "moved", 1, 9.0, 0.0),
    ];

    let rows = rows(&current, &base, &[file], &[], pass());

    assert_eq!(summary(&rows), [("moved", Change::Same, Some(9.0), false)]);
}

/// A new `impl From<Z> for A` inserted above the existing one: the existing
/// `A::from` keeps its base pair, and the inserted one is new.
#[test]
fn functions_of_one_name_pair_where_the_diff_carries_them() {
    let file = TouchedFile {
        diff: FileDiff {
            path: LIB.to_owned(),
            base_path: Some(LIB.to_owned()),
            hunks: vec![hunk(0, 0, 1, 9), hunk(3, 1, 12, 1)],
        },
        functions: BTreeMap::from([(1, function(8, "{ z }")), (10, function(13, "{ y }"))]),
    };
    let current = [
        entry(LIB, "A::from", 1, 2.0, 6.0),
        entry(LIB, "A::from", 10, 7.0, 56.0),
    ];
    let base = [entry(LIB, "A::from", 1, 6.0, 0.0)];

    let rows = rows(&current, &base, &[file], &[], pass());

    assert_eq!(
        summary(&rows),
        [
            ("A::from", Change::New, None, false),
            ("A::from", Change::Rose, Some(6.0), true),
        ]
    );
}

#[test]
fn a_function_whose_line_the_change_rewrote_pairs_with_the_one_of_its_name_left() {
    let current = [entry(LIB, "Thing::fmt", 1, 4.0, 20.0)];
    let base = [entry(LIB, "Thing::fmt", 5, 4.0, 0.0)];

    let rows = rows(&current, &base, &[touched_lib()], &[], pass());

    assert_eq!(
        summary(&rows),
        [("Thing::fmt", Change::Same, Some(4.0), false)]
    );
}

#[test]
fn a_renamed_function_pairs_with_the_removed_one_holding_the_same_body() {
    let current = [entry(LIB, "renamed", 1, 6.0, 42.0)];
    let base = [entry(LIB, "original", 5, 6.0, 0.0)];
    let base_files = [BaseFile {
        path: LIB.to_owned(),
        functions: BTreeMap::from([(5, function(10, "{ 1 }"))]),
    }];

    let rows = rows(&current, &base, &[touched_lib()], &base_files, pass());

    assert_eq!(
        summary(&rows),
        [("renamed", Change::Same, Some(6.0), false)]
    );
}

#[test]
fn a_function_moved_from_a_removed_file_pairs_by_its_body() {
    let current = [entry(LIB, "moved", 1, 6.0, 42.0)];
    let base = [
        entry("crates/a/src/gone.rs", "other", 1, 6.0, 0.0),
        entry("crates/a/src/gone.rs", "moved", 9, 6.0, 0.0),
    ];
    let base_files = [BaseFile {
        path: "crates/a/src/gone.rs".to_owned(),
        functions: BTreeMap::from([(1, function(7, "{ 7 }")), (9, function(12, "{ 1 }"))]),
    }];

    let rows = rows(&current, &base, &[touched_lib()], &base_files, pass());

    assert_eq!(summary(&rows), [("moved", Change::Same, Some(6.0), false)]);
}

#[test]
fn with_tests_failing_an_uncovered_function_is_unproven_not_failing() {
    let current = [
        entry(LIB, "uncovered", 1, 6.0, 42.0),
        entry(LIB, "too_complex", 22, 31.0, 992.0),
    ];
    let failing = Verdict {
        threshold: 30.0,
        tests_failed: true,
    };

    let rows = rows(&current, &[], &[touched_lib()], &[], failing);

    let verdicts: Vec<(&str, bool, bool, bool)> = rows
        .iter()
        .map(|row| {
            (
                row.name.as_str(),
                row.unproven,
                row.fails,
                row.coverage_uncertain,
            )
        })
        .collect();
    assert_eq!(
        verdicts,
        [
            ("uncovered", true, false, true),
            ("too_complex", false, true, true)
        ]
    );
}

#[test]
fn failing_rows_come_last_and_the_worst_last_among_them() {
    let current = [
        entry(LIB, "worst", 1, 9.0, 90.0),
        entry(LIB, "old", 12, 1.0, 1.0),
        entry(LIB, "bad", 22, 6.0, 42.0),
        entry(LIB, "kept", 42, 9.0, 90.0),
    ];
    let base = [entry(LIB, "kept", 42, 9.0, 0.0)];
    let mut file = touched_lib();
    file.diff.hunks = vec![hunk(1, 50, 1, 50)];

    let rows = rows(&current, &base, &[file], &[], pass());

    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["old", "kept", "bad", "worst"]);
}

#[test]
fn a_row_prints_as_one_json_object() {
    let current = [entry(LIB, "added", 1, 6.0, 42.0)];
    let failing = Verdict {
        threshold: 30.0,
        tests_failed: true,
    };

    let passed = rows(&current, &[], &[touched_lib()], &[], pass());
    let uncertain = rows(&current, &[], &[touched_lib()], &[], failing);

    assert_eq!(
        serde_json::to_string(&passed[0]).unwrap(),
        r#"{"path":"crates/a/src/lib.rs","name":"added","line":1,"complexity":6.0,"coverage":0.0,"score":42.0,"change":"new","base_complexity":null,"fails":true}"#
    );
    assert_eq!(
        serde_json::to_string(&uncertain[0]).unwrap(),
        r#"{"path":"crates/a/src/lib.rs","name":"added","line":1,"complexity":6.0,"coverage":0.0,"score":42.0,"change":"new","base_complexity":null,"fails":false,"unproven":true,"coverage_uncertain":true}"#
    );
}

/// Both `fn` lines rewritten, so neither pairs by line; cargo crap lists
/// the base's worse `A::from` first, and the pairs still follow the lines.
#[test]
fn functions_of_one_name_left_over_pair_in_line_order() {
    let file = TouchedFile {
        diff: FileDiff {
            path: LIB.to_owned(),
            base_path: Some(LIB.to_owned()),
            hunks: vec![hunk(10, 1, 10, 1), hunk(30, 1, 30, 1)],
        },
        functions: BTreeMap::from([(10, function(20, "{ a }")), (30, function(40, "{ b }"))]),
    };
    let current = [
        entry(LIB, "A::from", 10, 12.0, 156.0),
        entry(LIB, "A::from", 30, 3.0, 3.0),
    ];
    let base = [
        entry(LIB, "A::from", 30, 12.0, 12.0),
        entry(LIB, "A::from", 10, 2.0, 2.0),
    ];

    let rows = rows(&current, &base, &[file], &[], pass());

    assert_eq!(
        summary(&rows),
        [
            ("A::from", Change::Fell, Some(12.0), false),
            ("A::from", Change::Rose, Some(2.0), true),
        ]
    );
}
