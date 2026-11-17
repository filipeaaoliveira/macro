use factory_records::domain::models::Tampering;

use super::{ControlFiles, ControlFilesError, MAX_PATTERNS};

fn defaults() -> ControlFiles {
    ControlFiles::parse(&ControlFiles::default_patterns()).expect("the defaults parse")
}

fn paths(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|path| (*path).to_string()).collect()
}

/// The default list holds CI workflows, the checks file, agent
/// instructions, package scripts, test and lint configuration, and tests,
/// at the root or in a folder; other code is not a Control File.
#[test]
fn the_defaults_name_what_decides_whether_checks_pass() {
    let control_files = defaults();

    for path in [
        ".github/workflows/ci.yml",
        ".macro/checks.json",
        "AGENTS.md",
        "apps/web/AGENTS.md",
        "CLAUDE.md",
        "package.json",
        "apps/web/package.json",
        "biome.jsonc",
        "apps/web/tsconfig.app.json",
        "apps/web/vitest.config.ts",
        "apps/web/src/login.test.tsx",
        "crates/engine/src/flow/test.rs",
        "crates/engine/tests/flow.rs",
        "apps/web/.eslintrc.cjs",
    ] {
        assert!(control_files.matches(path), "{path} is a Control File");
    }
    for path in [
        "apps/web/src/login.tsx",
        "crates/engine/src/flow.rs",
        "README.md",
        "docs/testing.md",
        ".github/CODEOWNERS",
    ] {
        assert!(!control_files.matches(path), "{path} is not a Control File");
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

    let tampering = defaults().tampering(Some(&changed), "Fix the login bug");

    assert_eq!(
        tampering,
        Some(Tampering {
            files: paths(&[".github/workflows/ci.yml", "src/login.test.ts"]),
            unlisted: false,
        })
    );
}

/// A Control File the request names by its path is asked for: only the
/// others are Tampered, and a branch that changes none is not.
#[test]
fn a_control_file_the_request_names_is_asked_for() {
    let changed = paths(&["src/login.ts", ".github/workflows/ci.yml", "package.json"]);
    let request = "Run the login tests in CI\n\nAdd a job to .github/workflows/ci.yml.";

    let tampering = defaults().tampering(Some(&changed), request);

    assert_eq!(
        tampering,
        Some(Tampering {
            files: paths(&["package.json"]),
            unlisted: false,
        })
    );
    assert_eq!(
        defaults().tampering(Some(&paths(&["src/login.ts"])), request),
        None
    );
}

/// A request names a path only whole: not inside a longer path, nor by a
/// file name alone.
#[test]
fn a_request_names_a_path_only_whole() {
    let changed = paths(&["src/login.test.ts"]);
    let tampered = Some(Tampering {
        files: paths(&["src/login.test.ts"]),
        unlisted: false,
    });

    for request in [
        "Fix login.test.ts",
        "Fix app/src/login.test.ts",
        "Fix src/login.test.tsx",
        "Fix src/login.test.ts.snap",
    ] {
        assert_eq!(
            defaults().tampering(Some(&changed), request),
            tampered,
            "{request} does not name it"
        );
    }
    for request in [
        "Fix src/login.test.ts.",
        "Fix `src/login.test.ts`",
        "src/login.test.ts: fix it",
    ] {
        assert_eq!(
            defaults().tampering(Some(&changed), request),
            None,
            "{request} names it"
        );
    }
}

/// A branch whose changes GitHub could not list may have changed any
/// Control File: it is Tampered, unlisted. An empty list names no file.
#[test]
fn an_unlisted_branch_is_tampered_unless_the_list_is_empty() {
    assert_eq!(
        defaults().tampering(None, "Fix the login bug"),
        Some(Tampering {
            files: Vec::new(),
            unlisted: true,
        })
    );

    let none = ControlFiles::parse(&[]).expect("parses");
    assert_eq!(none.tampering(None, "Fix the login bug"), None);
    assert_eq!(
        none.tampering(Some(&paths(&[".github/workflows/ci.yml"])), "Fix it"),
        None
    );
}

/// A list too long, or a pattern that is empty, too long, absolute or not
/// a glob, is refused.
#[test]
fn a_list_that_does_not_hold_together_is_refused() {
    let too_many = vec!["*.rs".to_string(); MAX_PATTERNS + 1];
    assert_eq!(
        ControlFiles::parse(&too_many).err(),
        Some(ControlFilesError::TooMany)
    );

    for pattern in ["", "/ci.yml", "a**b", &"a".repeat(257)] {
        assert_eq!(
            ControlFiles::parse(&paths(&[pattern])).err(),
            Some(ControlFilesError::InvalidPattern(pattern.to_string())),
            "{pattern:?} is refused"
        );
    }
}

/// An approval covers the files it names; an unlisted Tampering only an
/// approval given while it was unlisted.
#[test]
fn an_approval_covers_the_files_it_names() {
    let tampering = Tampering {
        files: paths(&["package.json", "ci.yml"]),
        unlisted: false,
    };
    let unlisted = Tampering {
        files: Vec::new(),
        unlisted: true,
    };

    assert!(tampering.approved_by(&paths(&["ci.yml", "package.json"])));
    assert!(!tampering.approved_by(&paths(&["ci.yml"])));
    assert!(!tampering.approved_by(&[]));
    assert!(unlisted.approved_by(&[]));
    assert!(!unlisted.approved_by(&paths(&["ci.yml"])));
}
