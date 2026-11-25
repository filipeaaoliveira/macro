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

/// A repository whose base commit holds `crates/a/src/lib.rs` and
/// `crates/a/src/gone.rs`, and whose head changes `changed`, adds
/// `crates/a/src/added.rs`, a test file, a generated file and a file that
/// does not parse, and removes `gone.rs`.
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
    write(root, "crates/a/src/broken.rs", "fn broken( {\n");
    fs::remove_file(root.join("crates/a/src/gone.rs")).unwrap();
    commit_all(root, "head");
    (dir, base)
}

#[test]
fn without_a_complete_listing_every_changed_rust_source_file_is_touched() {
    let (dir, base) = repository();

    let touched = touched_files(dir.path(), &base, Listing::Incomplete).unwrap();

    let files: Vec<(FileDiff, Vec<(u32, u32)>)> = touched
        .files
        .into_iter()
        .map(|file| {
            let lasts = file
                .functions
                .iter()
                .map(|(first, function)| (*first, function.last));
            (file.diff, lasts.collect())
        })
        .collect();
    assert_eq!(
        files,
        [
            (
                FileDiff {
                    path: "crates/a/src/added.rs".to_owned(),
                    base_path: None,
                    hunks: vec![Hunk {
                        base_start: 0,
                        base_count: 0,
                        start: 1,
                        count: 3
                    }],
                },
                vec![(1, 3)],
            ),
            (
                FileDiff {
                    path: "crates/a/src/lib.rs".to_owned(),
                    base_path: Some("crates/a/src/lib.rs".to_owned()),
                    hunks: vec![Hunk {
                        base_start: 6,
                        base_count: 1,
                        start: 6,
                        count: 1
                    }],
                },
                vec![(1, 3), (5, 7)],
            ),
        ]
    );
    assert_eq!(touched.removed, ["crates/a/src/gone.rs"]);
}

#[test]
fn a_file_that_does_not_parse_is_warned_about_not_scored() {
    let (dir, base) = repository();

    let touched = touched_files(dir.path(), &base, Listing::Incomplete).unwrap();

    let warned: Vec<&str> = touched
        .warnings
        .iter()
        .map(|warning| warning.path.as_str())
        .collect();
    assert_eq!(warned, ["crates/a/src/broken.rs"]);
    assert!(
        touched.warnings[0]
            .warning
            .starts_with("not scored: it does not parse")
    );
}

#[test]
fn a_complete_listing_narrows_the_diff_to_its_paths() {
    let (dir, base) = repository();
    let listing = Listing::Complete(vec![
        "crates/a/src/lib.rs".to_owned(),
        "crates/a/src/gone.rs".to_owned(),
    ]);

    let touched = touched_files(dir.path(), &base, listing).unwrap();

    let paths: Vec<&str> = touched
        .files
        .iter()
        .map(|file| file.diff.path.as_str())
        .collect();
    assert_eq!(paths, ["crates/a/src/lib.rs"]);
    assert_eq!(touched.removed, ["crates/a/src/gone.rs"]);
}

#[test]
fn a_complete_listing_without_rust_source_touches_nothing() {
    let (dir, base) = repository();
    let listing = Listing::Complete(vec!["README.md".to_owned()]);

    let touched = touched_files(dir.path(), &base, listing).unwrap();

    assert!(touched.files.is_empty());
    assert!(touched.removed.is_empty());
}

#[test]
fn a_base_the_repository_holds_is_compared_with() {
    let (dir, base) = repository();
    let cli = Cli {
        base: Some(base.clone()),
    };

    assert_eq!(base_commit(&cli, dir.path()), Some(base));
}

#[test]
fn an_empty_or_unknown_base_skips_the_check() {
    let (dir, _) = repository();
    let empty = Cli {
        base: Some(String::new()),
    };
    let unknown = Cli {
        base: Some("0123456789abcdef0123456789abcdef01234567".to_owned()),
    };

    assert_eq!(base_commit(&empty, dir.path()), None);
    assert_eq!(base_commit(&unknown, dir.path()), None);
}

#[test]
fn the_summary_names_the_first_failed_tests() {
    let failed = ["a t::one", "a t::two", "a t::three", "a t::four"].map(str::to_owned);

    assert_eq!(failures(&[]), "");
    assert_eq!(
        failures(&failed),
        "; 4 tests failed (a t::one, a t::two, a t::three), so coverage may be low"
    );
}
