use super::*;
use crate::diff::Hunk;

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

/// `crates/a/src/lib.rs`, changed at lines 5 and 30, with functions at
/// 1-10, 12-20, 22-40 and 42-50.
fn touched_lib() -> TouchedFile {
    TouchedFile {
        diff: FileDiff {
            path: "crates/a/src/lib.rs".to_owned(),
            base_path: Some("crates/a/src/lib.rs".to_owned()),
            hunks: vec![
                Hunk { start: 5, count: 1 },
                Hunk {
                    start: 30,
                    count: 1,
                },
            ],
        },
        spans: BTreeMap::from([(1, 10), (12, 20), (22, 40), (42, 50)]),
    }
}

#[test]
fn only_touched_functions_get_rows() {
    let current = [
        entry("crates/a/src/lib.rs", "touched", 1, 2.0, 2.0),
        entry("crates/a/src/lib.rs", "untouched", 12, 9.0, 90.0),
        entry("crates/b/src/lib.rs", "elsewhere", 1, 9.0, 90.0),
    ];
    let base = [entry("crates/a/src/lib.rs", "touched", 1, 2.0, 0.0)];

    let rows = rows(&current, &base, &[touched_lib()], 30.0);

    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["touched"]);
}

#[test]
fn a_new_or_more_complex_function_above_the_threshold_fails() {
    let current = [
        entry("crates/a/src/lib.rs", "added", 1, 6.0, 42.0),
        entry("crates/a/src/lib.rs", "grown", 22, 7.0, 56.0),
    ];
    let base = [entry("crates/a/src/lib.rs", "grown", 20, 6.0, 0.0)];

    let rows = rows(&current, &base, &[touched_lib()], 30.0);

    assert_eq!(
        rows.iter()
            .map(|row| (
                row.name.as_str(),
                row.change,
                row.base_complexity,
                row.fails
            ))
            .collect::<Vec<_>>(),
        [
            ("added", Change::New, None, true),
            ("grown", Change::Rose, Some(6.0), true),
        ]
    );
}

#[test]
fn a_touched_function_no_more_complex_than_at_the_base_is_reported_without_failing() {
    let current = [
        entry("crates/a/src/lib.rs", "same", 1, 6.0, 42.0),
        entry("crates/a/src/lib.rs", "simpler", 22, 6.0, 42.0),
    ];
    let base = [
        entry("crates/a/src/lib.rs", "same", 1, 6.0, 0.0),
        entry("crates/a/src/lib.rs", "simpler", 22, 8.0, 0.0),
    ];

    let rows = rows(&current, &base, &[touched_lib()], 30.0);

    assert_eq!(
        rows.iter()
            .map(|row| (row.name.as_str(), row.change, row.score, row.fails))
            .collect::<Vec<_>>(),
        [
            ("same", Change::Same, 42.0, false),
            ("simpler", Change::Fell, 42.0, false)
        ]
    );
}

#[test]
fn a_new_function_at_or_below_the_threshold_passes() {
    let current = [entry("crates/a/src/lib.rs", "small", 1, 5.0, 30.0)];

    let rows = rows(&current, &[], &[touched_lib()], 30.0);

    assert!(!rows[0].fails);
    assert_eq!(rows[0].change, Change::New);
}

#[test]
fn a_main_function_gets_no_row() {
    let current = [entry("crates/a/src/lib.rs", "main", 1, 9.0, 90.0)];

    assert!(rows(&current, &[], &[touched_lib()], 30.0).is_empty());
}

#[test]
fn a_renamed_file_pairs_with_its_functions_under_the_earlier_name() {
    let mut file = touched_lib();
    file.diff.base_path = Some("crates/a/src/old.rs".to_owned());
    let current = [entry("crates/a/src/lib.rs", "moved", 1, 9.0, 90.0)];
    let base = [
        entry("crates/a/src/lib.rs", "moved", 1, 1.0, 0.0),
        entry("crates/a/src/old.rs", "moved", 1, 9.0, 0.0),
    ];

    let rows = rows(&current, &base, &[file], 30.0);

    assert_eq!(rows[0].change, Change::Same);
    assert!(!rows[0].fails);
}

#[test]
fn functions_of_one_name_pair_in_order() {
    let current = [
        entry("crates/a/src/lib.rs", "Thing::fmt", 1, 4.0, 20.0),
        entry("crates/a/src/lib.rs", "Thing::fmt", 22, 7.0, 56.0),
    ];
    let base = [
        entry("crates/a/src/lib.rs", "Thing::fmt", 1, 4.0, 0.0),
        entry("crates/a/src/lib.rs", "Thing::fmt", 18, 7.0, 0.0),
    ];

    let rows = rows(&current, &base, &[touched_lib()], 30.0);

    assert!(rows.iter().all(|row| row.change == Change::Same));
}

#[test]
fn failing_rows_come_last_and_the_worst_last_among_them() {
    let current = [
        entry("crates/a/src/lib.rs", "worst", 1, 9.0, 90.0),
        entry("crates/a/src/lib.rs", "old", 12, 1.0, 1.0),
        entry("crates/a/src/lib.rs", "bad", 22, 6.0, 42.0),
        entry("crates/a/src/lib.rs", "kept", 42, 9.0, 90.0),
    ];
    let base = [entry("crates/a/src/lib.rs", "kept", 42, 9.0, 0.0)];
    let mut file = touched_lib();
    file.diff.hunks = vec![Hunk {
        start: 1,
        count: 50,
    }];

    let rows = rows(&current, &base, &[file], 30.0);

    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["old", "kept", "bad", "worst"]);
}

#[test]
fn a_row_prints_as_one_json_object() {
    let current = [entry("crates/a/src/lib.rs", "added", 1, 6.0, 42.0)];

    let rows = rows(&current, &[], &[touched_lib()], 30.0);

    assert_eq!(
        serde_json::to_string(&rows[0]).unwrap(),
        r#"{"path":"crates/a/src/lib.rs","name":"added","line":1,"complexity":6.0,"coverage":0.0,"score":42.0,"change":"new","base_complexity":null,"fails":true}"#
    );
}
