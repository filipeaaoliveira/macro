//! Which changed files the check scores.
//!
//! A factory check is told the changed paths in the file `MACRO_CHECK_CHANGED`
//! names: its first line says whether the list is `complete`, and each line
//! after it is one path from the repository's root. Only a complete list
//! narrows the diff; otherwise the check diffs everything against the base.

#[cfg(test)]
mod test;

/// What the changed-paths file says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// Every path the change touched, removed files included.
    Complete(Vec<String>),
    /// Only some paths, or none: the diff against the base decides.
    Incomplete,
}

/// Reads the changed-paths file's text.
pub fn read_listing(text: &str) -> Listing {
    let mut lines = text.lines();
    if lines.next() != Some("complete") {
        return Listing::Incomplete;
    }

    let paths = lines
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect();
    Listing::Complete(paths)
}

/// Folders whose Rust files are tests, examples, generated code, migrations or
/// third-party code, which the check never scores.
const SKIPPED_FOLDERS: &[&str] = &[
    "tests",
    "test",
    "benches",
    "examples",
    "generated",
    "migrations",
    ".sqlx",
    "vendor",
    "target",
];

/// Files that hold test modules (`#[cfg(test)] mod test;`) or a build script.
/// `cargo crap` skips `#[cfg(test)]` items but cannot see the attribute on the
/// parent's `mod` line, so it would score the helpers in them as untested code.
const SKIPPED_FILES: &[&str] = &["test.rs", "tests.rs", "build.rs"];

/// Whether the check scores the file at `path`, from the repository's root.
///
/// It scores Rust source only: no tests, generated code, SQLx cache,
/// migrations, build scripts, or a service's `src/main.rs`, the startup code
/// that wires adapters and runs only with the service.
pub fn is_scored(path: &str) -> bool {
    let Some(folders) = path.strip_suffix(".rs") else {
        return false;
    };

    let mut parts: Vec<&str> = folders.split('/').collect();
    let file = parts.pop().unwrap_or_default();
    let skipped_folder = parts.iter().any(|part| SKIPPED_FOLDERS.contains(part));
    let skipped_file = SKIPPED_FILES.contains(&format!("{file}.rs").as_str());

    !skipped_folder && !skipped_file && !is_service_main(path)
}

/// Whether `path` is a service's `services/<name>/src/main.rs`.
fn is_service_main(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    matches!(parts.as_slice(), ["services", _, "src", "main.rs"])
}

/// How many lines at a file's top a generated-code marker is looked for in.
const GENERATED_MARKER_LINES: usize = 5;

/// Whether a file's text says it is generated (`@generated` near its top).
pub fn is_generated(source: &str) -> bool {
    source
        .lines()
        .take(GENERATED_MARKER_LINES)
        .any(|line| line.contains("@generated"))
}
