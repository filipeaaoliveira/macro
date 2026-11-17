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
        .for_changes(changed.as_deref(), &[])
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
        names(
            &checks,
            Some(&["packages/ui/button.tsx", "crates/app/lib.rs"])
        ),
        ["web", "rust", "lint"]
    );
    // A folder's name is not a prefix of another folder's.
    assert_eq!(
        names(
            &checks,
            Some(&["apps/website/index.ts", "crates/application/lib.rs"])
        ),
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
        (
            r#"{"checks":[{"name":"web","run":"x","timeout":0}]}"#,
            "timeout",
        ),
        (
            r#"{"checks":[{"name":"web","run":"x","timeout":1201}]}"#,
            "timeout",
        ),
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
            ChecksFileError::Timeout(_) => "timeout",
            ChecksFileError::Path { .. } => "path",
            ChecksFileError::TooLarge => "too large",
        };
        assert_eq!(kind, expected, "{text}");
    }
}

#[test]
fn a_path_is_a_file_or_folder_from_the_root() {
    for path in [
        "",
        ".",
        "./apps",
        "/apps",
        "apps/../crates",
        "apps//web",
        "apps/*",
        "*.rs",
        "apps/{web,ui}",
        "src/[ab]",
    ] {
        let text = format!(r#"{{"checks":[{{"name":"web","run":"x","paths":[{path:?}]}}]}}"#);
        let error = RepositoryChecks::parse(&text).expect_err(path);
        assert!(
            matches!(error, ChecksFileError::Path { .. }),
            "{path}: {error}"
        );
    }
    for path in ["apps/web", "apps/web/", "Cargo.lock", ".github/workflows"] {
        let text = format!(r#"{{"checks":[{{"name":"web","run":"x","paths":[{path:?}]}}]}}"#);
        RepositoryChecks::parse(&text).expect(path);
    }
}

#[test]
fn a_kept_check_runs_whatever_changed() {
    let checks = RepositoryChecks::parse(CHECKS).expect("parse");
    let changed = vec!["README.md".to_string()];

    let kept: Vec<&str> = checks
        .for_changes(Some(&changed), &["rust".to_string()])
        .into_iter()
        .map(|check| check.name.as_str())
        .collect();

    assert_eq!(kept, ["rust", "lint"]);
}

#[test]
fn a_file_over_what_a_round_keeps_does_not_read() {
    let text = format!(r#"{{"checks":[],"x":"{}"}}"#, "a".repeat(64 * 1024));

    assert_eq!(
        RepositoryChecks::parse(&text),
        Err(ChecksFileError::TooLarge)
    );
}

/// A check takes the timeout it declares, or 20 minutes.
#[test]
fn a_check_s_timeout_defaults_to_twenty_minutes() {
    let checks = RepositoryChecks::parse(
        r#"{"checks":[{"name":"web","run":"x","timeout":90},{"name":"lint","run":"y"}]}"#,
    )
    .expect("reads");

    let timeouts: Vec<u64> = checks.checks.iter().map(|check| check.timeout).collect();
    assert_eq!(timeouts, [90, 1200]);
}
