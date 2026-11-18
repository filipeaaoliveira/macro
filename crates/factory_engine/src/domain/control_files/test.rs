use factory_records::domain::models::ControlFileChanges;

use super::{ControlFiles, ControlFilesError, MAX_PATTERNS};

fn defaults() -> ControlFiles {
    ControlFiles::parse(&ControlFiles::default_patterns()).expect("the defaults parse")
}

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

fn tampered(files: &[&str]) -> Option<ControlFileChanges> {
    Some(ControlFileChanges {
        tampered: paths(files),
        named: Vec::new(),
        unlisted: false,
    })
}

fn unlisted() -> Option<ControlFileChanges> {
    Some(ControlFileChanges {
        tampered: Vec::new(),
        named: Vec::new(),
        unlisted: true,
    })
}

/// The default list holds CI workflows, the checks and verify files, agent
/// instructions and their tools' settings, package and build scripts, test
/// and lint configuration, and tests with what they compare against, at the
/// root or in a folder; other code is not a Control File.
#[test]
fn the_defaults_name_what_decides_whether_checks_pass() {
    let control_files = defaults();

    for path in [
        ".github/workflows/ci.yml",
        ".macro/checks.json",
        ".macro/verify.json",
        "AGENTS.md",
        "apps/web/AGENTS.md",
        "CLAUDE.md",
        ".claude/settings.json",
        ".agents/rules/rust.md",
        ".cursor/rules/web.mdc",
        "package.json",
        "apps/web/package.json",
        "Cargo.toml",
        "crates/engine/Cargo.toml",
        "crates/engine/build.rs",
        "rust-toolchain.toml",
        "scripts/checks.sh",
        ".npmrc",
        "pnpm-workspace.yaml",
        "turbo.json",
        "apps/web/vitest.workspace.ts",
        "crates/engine/src/flow_test.rs",
        "GNUmakefile",
        ".config/nextest.toml",
        "deny.toml",
        "biome.jsonc",
        "biome.base.jsonc",
        "apps/web/tsconfig.app.json",
        "apps/web/vite.config.ts",
        "apps/web/vitest.config.ts",
        "apps/web/src/login.test.tsx",
        "crates/engine/src/flow/test.rs",
        "crates/engine/src/flow/tests.rs",
        "crates/engine/tests/flow.rs",
        "crates/engine/src/snapshots/flow.snap",
        "apps/web/src/__snapshots__/login.snap",
        "crates/records/src/fixtures/people.sql",
        "tools/conftest.py",
        "apps/web/.eslintrc.cjs",
    ] {
        assert!(control_files.matches(path), "{path} is a Control File");
    }
    for path in [
        "apps/web/src/login.tsx",
        "crates/engine/src/flow.rs",
        "crates/engine/src/test_support.rs",
        "README.md",
        "docs/testing.md",
        ".github/CODEOWNERS",
    ] {
        assert!(!control_files.matches(path), "{path} is not a Control File");
    }
}

/// A path matches whatever its case: a tool reads `makefile` as it reads
/// `Makefile`.
#[test]
fn a_path_matches_whatever_its_case() {
    let control_files = defaults();

    for path in [
        "makefile",
        "Justfile",
        "tools/agents.md",
        ".GitHub/Workflows/ci.yml",
    ] {
        assert!(control_files.matches(path), "{path} is a Control File");
    }
}

/// `*` stays within one folder; `**` spans any number.
#[test]
fn a_single_star_stays_within_one_folder() {
    let control_files = ControlFiles::parse(&paths(&["ci/*.yml"])).expect("parses");

    assert!(control_files.matches("ci/build.yml"));
    assert!(!control_files.matches("ci/nested/build.yml"));
}

/// A branch that changes Control Files its request does not name is
/// Tampered with them, in GitHub's order, once each.
#[test]
fn a_branch_that_changes_control_files_is_tampered_with_them() {
    let changed = paths(&[
        "src/login.ts",
        ".github/workflows/ci.yml",
        "src/login.test.ts",
        ".github/workflows/ci.yml",
    ]);

    let changes = defaults().changes(Some(&changed), "Fix the login bug", true);

    assert_eq!(
        changes,
        tampered(&[".github/workflows/ci.yml", "src/login.test.ts"])
    );
}

/// A Run created in person asks for the Control Files its request names by
/// path: they are kept as named, and only the others are Tampered. A branch
/// that changes only named ones is not Tampered.
#[test]
fn a_control_file_an_in_person_request_names_is_asked_for() {
    let changed = paths(&["src/login.ts", ".github/workflows/ci.yml", "package.json"]);
    let request = "Run the login tests in CI\n\nAdd a job to .github/workflows/ci.yml.";

    let changes = defaults().changes(Some(&changed), request, true);
    let only_named = defaults()
        .changes(Some(&paths(&[".github/workflows/ci.yml"])), request, true)
        .expect("it changed a Control File");

    assert_eq!(
        changes,
        Some(ControlFileChanges {
            tampered: paths(&["package.json"]),
            named: paths(&[".github/workflows/ci.yml"]),
            unlisted: false,
        })
    );
    assert!(!only_named.is_tampered());
    assert_eq!(
        defaults().changes(Some(&paths(&["src/login.ts"])), request, true),
        None
    );
}

/// A Run not created in person, as by an agent, asks for nothing: a file
/// its request names is Tampered all the same.
#[test]
fn a_request_not_made_in_person_asks_for_nothing() {
    let changed = paths(&[".github/workflows/ci.yml"]);
    let request = "Add a job to .github/workflows/ci.yml.";

    let changes = defaults().changes(Some(&changed), request, false);

    assert_eq!(changes, tampered(&[".github/workflows/ci.yml"]));
}

/// A request names a path only whole: not inside a longer path, nor by a
/// file name alone.
#[test]
fn a_request_names_a_path_only_whole() {
    let changed = paths(&["src/login.test.ts"]);

    for request in [
        "Fix login.test.ts",
        "Fix app/src/login.test.ts",
        "Fix src/login.test.tsx",
        "Fix src/login.test.ts.snap",
    ] {
        assert_eq!(
            defaults().changes(Some(&changed), request, true),
            tampered(&["src/login.test.ts"]),
            "{request} does not name it"
        );
    }
    for request in [
        "Fix src/login.test.ts.",
        "Fix `src/login.test.ts`",
        "src/login.test.ts: fix it",
    ] {
        let changes = defaults()
            .changes(Some(&changed), request, true)
            .expect("it changed a Control File");
        assert!(!changes.is_tampered(), "{request} names it");
    }
}

/// A branch whose changes GitHub could not list may have changed any
/// Control File: it is Tampered, unlisted. An empty list names no file.
#[test]
fn an_unlisted_branch_is_tampered_unless_the_list_is_empty() {
    assert_eq!(
        defaults().changes(None, "Fix the login bug", true),
        unlisted()
    );

    let none = ControlFiles::parse(&[]).expect("parses");
    assert_eq!(none.changes(None, "Fix the login bug", true), None);
    assert_eq!(
        none.changes(Some(&paths(&[".github/workflows/ci.yml"])), "Fix it", true),
        None
    );
}

/// More Tampered files than a Run keeps, as a rename lists both of its
/// paths, read as unlisted rather than as a list too long to keep.
#[test]
fn more_control_files_than_a_run_keeps_are_unlisted() {
    let max = ControlFileChanges::MAX_FILES;
    let at_most: Vec<String> = (0..max).map(|n| format!("tests/case{n}.rs")).collect();
    let more: Vec<String> = (0..=max).map(|n| format!("tests/case{n}.rs")).collect();

    let kept = defaults()
        .changes(Some(&at_most), "Rename the cases", true)
        .expect("it changed Control Files");

    assert_eq!(kept.tampered.len(), max);
    assert!(!kept.unlisted);
    assert_eq!(
        defaults().changes(Some(&more), "Rename the cases", true),
        unlisted()
    );
}

/// A list too long, or a pattern that is empty, too long, starts at the
/// root or `./`, or is not a glob, is refused.
#[test]
fn a_list_that_does_not_hold_together_is_refused() {
    let too_many = vec!["*.rs".to_string(); MAX_PATTERNS + 1];
    assert_eq!(
        ControlFiles::parse(&too_many).err(),
        Some(ControlFilesError::TooMany)
    );

    for pattern in ["", "/ci.yml", "./ci.yml", "a**b", &"a".repeat(257)] {
        assert_eq!(
            ControlFiles::parse(&paths(&[pattern])).err(),
            Some(ControlFilesError::InvalidPattern(pattern.to_string())),
            "{pattern:?} is refused"
        );
    }
}

/// An approval covers the Tampered files it names; unlisted changes only an
/// approval given while they were unlisted.
#[test]
fn an_approval_covers_the_files_it_names() {
    let changes = ControlFileChanges {
        tampered: paths(&["package.json", "ci.yml"]),
        named: paths(&["Makefile"]),
        unlisted: false,
    };
    let unlisted = ControlFileChanges {
        tampered: Vec::new(),
        named: Vec::new(),
        unlisted: true,
    };

    assert!(changes.approved_by(&paths(&["ci.yml", "package.json"])));
    assert!(!changes.approved_by(&paths(&["ci.yml"])));
    assert!(!changes.approved_by(&[]));
    assert!(unlisted.approved_by(&[]));
    assert!(!unlisted.approved_by(&paths(&["ci.yml"])));
}
