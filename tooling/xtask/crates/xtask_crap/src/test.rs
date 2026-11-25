use std::collections::BTreeMap;

use tempfile::TempDir;
use xtask_crap::diff::Hunk;

use super::*;

const BASE_LIB: &str = "\
fn kept() -> u8 {
    1
}

fn changed() -> u8 {
    2
}
";

const HEAD_LIB: &str = "\
fn kept() -> u8 {
    1
}

fn changed() -> u8 {
    3
}
";

const ADDED: &str = "\
fn added() -> u8 {
    4
}
";

fn commit_all(root: &Path, message: &str) -> String {
    for args in [
        vec!["add", "--all"],
        vec![
            "-c",
            "user.name=check",
            "-c",
            "user.email=check@example.com",
            "commit",
            "--quiet",
            "--message",
            message,
        ],
    ] {
        output(git(root).args(args)).unwrap();
    }
    output(git(root).args(["rev-parse", "HEAD"]))
        .unwrap()
        .trim()
        .to_owned()
}

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

/// A repository whose base commit holds `crates/a/src/lib.rs`, and whose
/// head changes `changed`, adds `crates/a/src/added.rs`, a test file and a
/// generated file, and removes `crates/a/src/gone.rs`.
fn repository() -> (TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    output(git(root).args(["init", "--quiet"])).unwrap();
    write(root, "crates/a/src/lib.rs", BASE_LIB);
    write(root, "crates/a/src/gone.rs", "fn gone() {}\n");
    let base = commit_all(root, "base");

    write(root, "crates/a/src/lib.rs", HEAD_LIB);
    write(root, "crates/a/src/added.rs", ADDED);
    write(root, "crates/a/src/test.rs", ADDED);
    write(
        root,
        "crates/a/src/schema.rs",
        "// @generated\nfn schema() {}\n",
    );
    fs::remove_file(root.join("crates/a/src/gone.rs")).unwrap();
    commit_all(root, "head");
    (dir, base)
}

#[test]
fn without_a_complete_listing_every_changed_rust_source_file_is_touched() {
    let (dir, base) = repository();

    let files = touched_files(dir.path(), &base, Listing::Incomplete).unwrap();

    let touched: Vec<_> = files
        .into_iter()
        .map(|file| (file.diff, file.spans))
        .collect();
    assert_eq!(
        touched,
        [
            (
                FileDiff {
                    path: "crates/a/src/added.rs".to_owned(),
                    base_path: None,
                    hunks: vec![Hunk { start: 1, count: 3 }],
                },
                BTreeMap::from([(1, 3)]),
            ),
            (
                FileDiff {
                    path: "crates/a/src/lib.rs".to_owned(),
                    base_path: Some("crates/a/src/lib.rs".to_owned()),
                    hunks: vec![Hunk { start: 6, count: 1 }],
                },
                BTreeMap::from([(1, 3), (5, 7)]),
            ),
        ]
    );
}

#[test]
fn a_complete_listing_narrows_the_diff_to_its_paths() {
    let (dir, base) = repository();
    let listing = Listing::Complete(vec![
        "crates/a/src/lib.rs".to_owned(),
        "crates/a/src/gone.rs".to_owned(),
    ]);

    let files = touched_files(dir.path(), &base, listing).unwrap();

    let paths: Vec<&str> = files.iter().map(|file| file.diff.path.as_str()).collect();
    assert_eq!(paths, ["crates/a/src/lib.rs"]);
}

#[test]
fn a_complete_listing_without_rust_source_touches_nothing() {
    let (dir, base) = repository();
    let listing = Listing::Complete(vec!["README.md".to_owned()]);

    assert!(
        touched_files(dir.path(), &base, listing)
            .unwrap()
            .is_empty()
    );
}
