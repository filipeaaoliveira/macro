//! `cargo x crap-rust [--base <commit>]`
//!
//! Scores the Rust functions the change since the base commit touched, and
//! fails when one is new or more complex than at the base and scores above
//! the threshold in `.cargo-crap.toml`.
//!
//! It keeps the changed Rust source files (no tests, generated code,
//! migrations or service startup code), runs the tests of the packages that
//! hold them under coverage (`cargo llvm-cov nextest`), scores those packages
//! (`cargo crap`), and scores the base's versions of the files for their
//! complexity. It prints one JSON line per touched function, those that fail
//! last, then a summary line. Exit code: 0 passed, 1 a function failed, 2 the
//! check could not run.
//!
//! The base is `--base`, else `MACRO_CHECK_BASE`. The changed paths are the
//! ones `MACRO_CHECK_CHANGED`'s file lists when its first line is `complete`,
//! else every path `git diff <base> HEAD` names.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;
use rootcause::prelude::*;
use serde::Deserialize;
use xtask_crap::changed::{self, Listing};
use xtask_crap::diff::{self, FileDiff};
use xtask_crap::packages::Metadata;
use xtask_crap::rows::{self, Entry, Report as Scores, Row, TouchedFile};
use xtask_crap::spans;

#[derive(Parser)]
#[command(
    name = "crap-rust",
    about = "CRAP scores of the Rust functions a change touched"
)]
struct Cli {
    /// The commit the change is made against. Defaults to `MACRO_CHECK_BASE`.
    #[arg(long)]
    base: Option<String>,
}

/// The variable naming the commit the change is made against.
const BASE_VARIABLE: &str = "MACRO_CHECK_BASE";
/// The variable naming the file that lists the changed paths.
const CHANGED_VARIABLE: &str = "MACRO_CHECK_CHANGED";
/// `cargo crap`'s settings, at the repository's root.
const CONFIG: &str = ".cargo-crap.toml";
/// Files holding test modules, which `cargo crap` would score as untested
/// code (see [`changed::is_scored`]).
const TEST_FILES: [&str; 6] = [
    "--exclude",
    "**/test.rs",
    "--exclude",
    "**/tests.rs",
    "--exclude",
    "**/test/**",
];
/// The exit code when the check could not run.
const NOT_RUN: u8 = 2;

fn main() -> ExitCode {
    match check(&Cli::parse()) {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::FAILURE,
        Err(report) => {
            eprintln!("crap-rust: {report}");
            ExitCode::from(NOT_RUN)
        }
    }
}

/// Runs the check; true when a function fails it.
fn check(cli: &Cli) -> Result<bool, Report> {
    let root = xtask_paths::repo_root();
    let base = base_commit(cli)?;
    let files = touched_files(&root, &base)?;
    if files.is_empty() {
        println!("crap-rust: the change touched no scored Rust source");
        return Ok(false);
    }

    let work = tempfile::tempdir().context("could not create a work folder")?;
    let current = score_change(&root, &files, work.path())?;
    let at_base = score_base(&root, &base, &files, &work.path().join("base"))?;
    let threshold = threshold(&root)?;
    print(
        &rows::rows(&current, &at_base, &files, threshold),
        threshold,
    )
}

fn base_commit(cli: &Cli) -> Result<String, Report> {
    let base = cli
        .base
        .clone()
        .or_else(|| std::env::var_os(BASE_VARIABLE).and_then(|value| value.into_string().ok()))
        .filter(|base| !base.is_empty());
    base.ok_or_else(|| report!("no base commit: pass --base or set {BASE_VARIABLE}"))
}

/// What the changed-paths file says; incomplete when there is none.
fn listing() -> Listing {
    std::env::var_os(CHANGED_VARIABLE)
        .and_then(|path| fs::read_to_string(path).ok())
        .map_or(Listing::Incomplete, |text| changed::read_listing(&text))
}

/// The scored files the change left in the tree, with their diffs and their
/// functions' spans.
fn touched_files(root: &Path, base: &str) -> Result<Vec<TouchedFile>, Report> {
    let pathspec: Vec<String> = match listing() {
        Listing::Complete(paths) => paths
            .into_iter()
            .filter(|path| changed::is_scored(path))
            .collect(),
        Listing::Incomplete => vec!["*.rs".to_owned()],
    };
    if pathspec.is_empty() {
        return Ok(Vec::new());
    }

    let text = output(
        git(root)
            .args([
                "diff",
                "--unified=0",
                "--no-prefix",
                "--find-renames",
                "--no-color",
                "--no-ext-diff",
                base,
                "HEAD",
                "--",
            ])
            .args(&pathspec),
    )?;

    let mut files = Vec::new();
    for diff in diff::parse(&text) {
        if changed::is_scored(&diff.path) {
            files.extend(touched_file(root, diff)?);
        }
    }
    Ok(files)
}

/// The file with its functions' spans; none when it is generated code.
fn touched_file(root: &Path, diff: FileDiff) -> Result<Option<TouchedFile>, Report> {
    let source = fs::read_to_string(root.join(&diff.path))
        .context(format!("could not read {}", diff.path))?;
    if changed::is_generated(&source) {
        return Ok(None);
    }

    let spans = spans::function_spans(&source)
        .map_err(|error| report!("could not parse {}: {error}", diff.path))?;
    Ok(Some(TouchedFile { diff, spans }))
}

/// The scores of every function in the packages holding the touched files,
/// against those packages' own tests.
fn score_change(root: &Path, files: &[TouchedFile], work: &Path) -> Result<Vec<Entry>, Report> {
    let metadata: Metadata = serde_json::from_str(&output(cargo(root).args([
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
    ]))?)
    .context("could not read cargo metadata")?;
    let paths: Vec<String> = files.iter().map(|file| file.diff.path.clone()).collect();
    let packages: Vec<String> = metadata.packages_of(&paths).into_keys().collect();
    if packages.is_empty() {
        return Ok(Vec::new());
    }

    let selection: Vec<&str> = packages
        .iter()
        .flat_map(|name| ["-p", name.as_str()])
        .collect();
    let lcov = work.join("lcov.info");
    let report = work.join("change.json");

    // Profiles an earlier run left would count as this run's coverage. A
    // failing test is the tests' check's to report: the coverage of the
    // others still scores.
    run(cargo(root).args(["llvm-cov", "clean", "--profraw-only"]))?;
    run(cargo(root)
        .args([
            "llvm-cov",
            "nextest",
            "--no-clean",
            "--no-fail-fast",
            "--ignore-run-fail",
        ])
        .args(&selection)
        .arg("--lcov")
        .arg("--output-path")
        .arg(&lcov))?;
    run(cargo(root)
        .args(["crap", "--format", "json"])
        .args(&selection)
        .args(TEST_FILES)
        .arg("--lcov")
        .arg(&lcov)
        .arg("--output")
        .arg(&report))?;
    read_scores(&report)
}

/// The complexities of every function in the touched files' base versions.
fn score_base(
    root: &Path,
    base: &str,
    files: &[TouchedFile],
    tree: &Path,
) -> Result<Vec<Entry>, Report> {
    let base_paths: Vec<&str> = files
        .iter()
        .filter_map(|file| file.diff.base_path.as_deref())
        .collect();
    if base_paths.is_empty() {
        return Ok(Vec::new());
    }

    // `cargo crap` reads its settings from the folder it runs in, so the
    // base's scores count complexity as the change's do.
    fs::create_dir_all(tree).context("could not create the base's folder")?;
    fs::copy(root.join(CONFIG), tree.join(CONFIG)).context(format!("could not copy {CONFIG}"))?;
    for path in base_paths {
        let source = output(git(root).arg("show").arg(format!("{base}:{path}")))?;
        let copy = tree.join(path);
        fs::create_dir_all(copy.parent().unwrap_or(tree))
            .context(format!("could not copy {path}"))?;
        fs::write(&copy, source).context(format!("could not copy {path}"))?;
    }

    let report = tree.join("base.json");
    run(cargo(tree)
        .args(["crap", "--format", "json", "--path", "."])
        .args(TEST_FILES)
        .arg("--output")
        .arg(&report))?;

    // Without `-p`, paths start from `--path`.
    let mut entries = read_scores(&report)?;
    for entry in &mut entries {
        entry.file = entry.file.trim_start_matches("./").to_owned();
    }
    Ok(entries)
}

fn read_scores(path: &Path) -> Result<Vec<Entry>, Report> {
    let text = fs::read_to_string(path).context(format!("could not read {}", path.display()))?;
    let scores: Scores =
        serde_json::from_str(&text).context("could not read cargo crap's report")?;
    Ok(scores.entries)
}

#[derive(Deserialize)]
struct Config {
    threshold: f64,
}

/// The score above which a function fails, from `.cargo-crap.toml`.
fn threshold(root: &Path) -> Result<f64, Report> {
    let text = fs::read_to_string(root.join(CONFIG)).context(format!("could not read {CONFIG}"))?;
    let config: Config = toml::from_str(&text).context(format!("{CONFIG} has no threshold"))?;
    Ok(config.threshold)
}

/// Prints the rows and a summary; true when a row fails.
fn print(rows: &[Row], threshold: f64) -> Result<bool, Report> {
    for row in rows {
        println!(
            "{}",
            serde_json::to_string(row).context("could not write a row")?
        );
    }

    let failing = rows.iter().filter(|row| row.fails).count();
    let others = rows
        .iter()
        .filter(|row| !row.fails && row.score > threshold)
        .count();
    println!(
        "crap-rust: {} touched functions; {failing} new or more complex above {threshold} (failing); {others} others above it",
        rows.len()
    );
    Ok(failing > 0)
}

fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args(["-c", "core.quotePath=false"]);
    command
}

/// Cargo run in `folder`: the cargo that started this command, when one did.
fn cargo(folder: &Path) -> Command {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(PathBuf::from(cargo));
    command.current_dir(folder);
    command
}

/// Runs `command`, its output going to this command's error stream so the
/// rows stay last.
fn run(command: &mut Command) -> Result<(), Report> {
    let status = command
        .stdout(std::io::stderr())
        .status()
        .context(format!("could not run {command:?}"))?;
    if !status.success() {
        bail!("{command:?} failed with {status}");
    }
    Ok(())
}

fn output(command: &mut Command) -> Result<String, Report> {
    let output = command
        .output()
        .context(format!("could not run {command:?}"))?;
    if !output.status.success() {
        bail!(
            "{command:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let text = String::from_utf8(output.stdout)
        .context(format!("{command:?} wrote text that is not UTF-8"))?;
    Ok(text)
}
