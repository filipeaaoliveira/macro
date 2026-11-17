use super::{ChecksFileError, RepositoryChecks};

const CHECKS: &str = r#"{
    "checks": [
        { "name": "web", "run": "bun run check", "paths": ["apps/web/", "packages/ui"] },
        { "name": "rust", "run": "cargo test -p app", "paths": ["crates/app", "Cargo.lock"] },
        { "name": "lint", "run": "just check" }
    ]
}"#;

fn names(checks: &RepositoryChecks, changed: Option<&[&str]>) -> Vec<String> {
    let changed: Option<Vec<String>> =
        changed.map(|paths| paths.iter().map(|path| path.to_string()).collect());
    checks
        .for_changes(changed.as_deref())
        .into_iter()
        .map(|check| check.name.clone())
        .collect()
}

#[test]
fn a_check_runs_when_the_branch_changed_a_path_it_names() {
    let checks = RepositoryChecks::parse(CHECKS).expect("parse");

    assert_eq!(checks.rounds, 3);
    assert_eq!(
        names(&checks, Some(&["apps/web/src/main.ts"])),
        ["web", "lint"]
    );
    assert_eq!(names(&checks, Some(&["Cargo.lock"])), ["rust", "lint"]);
    assert_eq!(
        names(&checks, Some(&["packages/ui/button.tsx", "crates/app/lib.rs"])),
        ["web", "rust", "lint"]
    );
    // A folder's name is not a prefix of another folder's.
    assert_eq!(
        names(&checks, Some(&["apps/website/index.ts", "crates/application/lib.rs"])),
        ["lint"]
    );
}

#[test]
fn every_check_runs_when_the_changed_paths_are_not_known() {
    let checks = RepositoryChecks::parse(CHECKS).expect("parse");

    assert_eq!(names(&checks, None), ["web", "rust", "lint"]);
}

#[test]
fn a_file_that_breaks_the_rules_says_why() {
    let cases = [
        ("{", "unreadable"),
        (r#"{"checks":[],"extra":1}"#, "unreadable"),
        (r#"{"rounds":0,"checks":[]}"#, "rounds"),
        (r#"{"rounds":11,"checks":[]}"#, "rounds"),
        (r#"{"checks":[{"name":"Web","run":"x"}]}"#, "name"),
        (
            r#"{"checks":[{"name":"web","run":"x"},{"name":"web","run":"y"}]}"#,
            "duplicate",
        ),
        (r#"{"checks":[{"name":"web","run":"  "}]}"#, "run"),
    ];

    for (text, expected) in cases {
        let error = RepositoryChecks::parse(text).expect_err(text);
        let kind = match error {
            ChecksFileError::Unreadable(_) => "unreadable",
            ChecksFileError::Rounds(_) => "rounds",
            ChecksFileError::TooMany(_) => "too many",
            ChecksFileError::Name(_) => "name",
            ChecksFileError::DuplicateName(_) => "duplicate",
            ChecksFileError::Run(_) => "run",
        };
        assert_eq!(kind, expected, "{text}");
    }
}
