//! `xtask_crap [--base <commit>]`, `cargo x crap-rust` in this repository.
//!
//! Scores the Rust functions the change since the base commit touched, and
//! fails when one is new or more complex than at the base and scores above
//! the threshold in `.cargo-crap.toml` (30 without one).
//!
//! It keeps the changed Rust source files (no tests, generated code,
//! migrations or service startup code), runs the tests of the packages that
//! hold them under coverage (`cargo llvm-cov nextest`), scores those packages
//! (`cargo crap`), and scores the base's versions of the files for their
//! complexity. It prints a warning line per changed file it cannot score, one
//! JSON line per touched function, those that fail last, then a summary
//! line. Exit code: 0 passed or skipped, 1 a function failed, 2 the check
//! could not run.
//!
//! The base is `--base`, else `MACRO_CHECK_BASE`; without one in the
//! repository the check is skipped. The changed paths are the ones
//! `MACRO_CHECK_CHANGED`'s file lists when its first line is `complete`,
//! else every path `git diff <base> HEAD` names. It runs from the root of a
//! repository whose Cargo workspace is at its root.

use std::ffi::OsString;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;
use rootcause::prelude::*;
use serde::{Deserialize, Serialize};
use xtask_crap::changed::{self, Listing};
use xtask_crap::diff::{self, FileDiff};
use xtask_crap::nextest;
use xtask_crap::packages::Metadata;
use xtask_crap::rows::{self, BaseFile, Entry, Report as Scores, Row, TouchedFile, Verdict};
use xtask_crap::spans;

#[cfg(test)]
mod test;

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
/// The threshold without a `.cargo-crap.toml`: cargo crap's own.
const DEFAULT_THRESHOLD: f64 = 30.0;
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
/// The cargo subcommands the check runs, and how to install them.
const TOOLS: [&str; 3] = ["llvm-cov", "nextest", "crap"];
const INSTALL_HINT: &str = "install them with `cargo install --locked cargo-llvm-cov cargo-nextest cargo-crap` and `rustup component add llvm-tools-preview`";
/// The nextest profile the coverage run uses, from [`NEXTEST_CONFIG`]. It
/// inherits the repository's default profile.
const NEXTEST_PROFILE: &str = "default-crap-rust";
/// A test still running after three minutes is ended, so one that hangs
/// cannot take the round's time.
const NEXTEST_CONFIG: &str = "\
[profile.default-crap-rust]
slow-timeout = { period = \"60s\", terminate-after = 3 }
";
/// Where coverage builds go when neither `CARGO_LLVM_COV_TARGET_DIR` nor
/// `CARGO_TARGET_DIR` says, under the user's cache: outside the worktree,
/// so a later round's build is warm.
const COVERAGE_TARGET: &str = "macro-check/crap-target";
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

/// A changed file the check could not score, printed as a JSON line.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Warning {
    path: String,
    warning: String,
}

/// What the change touched.
#[derive(Debug, Default)]
struct Touched {
    files: Vec<TouchedFile>,
    /// The base paths of scored files the change removed.
    removed: Vec<String>,
    warnings: Vec<Warning>,
}

/// The change's scores.
struct Scored {
    entries: Vec<Entry>,
    failed_tests: Vec<String>,
}

/// Runs the check; true when a function fails it.
fn check(cli: &Cli) -> Result<bool, Report> {
    let root = repo_root()?;
    let Some(base) = base_commit(cli, &root) else {
        println!("crap-rust: skipped: no base commit to compare with");
        return Ok(false);
    };

    require_tools(&root)?;
    let mut touched = touched_files(&root, &base, listing())?;
    let work = tempfile::tempdir().context("could not create a work folder")?;
    let scored = score_change(&root, &mut touched, work.path())?;
    let base_files = base_files(&root, &base, &touched, &work.path().join("base"))?;
    let at_base = score_base(&base_files, &work.path().join("base"))?;

    let verdict = Verdict {
        threshold: threshold(&root)?,
        tests_failed: !scored.failed_tests.is_empty(),
    };
    let rows = rows::rows(
        &scored.entries,
        &at_base,
        &touched.files,
        &base_files,
        verdict,
    );
    print(
        &touched.warnings,
        &rows,
        &scored.failed_tests,
        verdict.threshold,
    )
}

fn repo_root() -> Result<PathBuf, Report> {
    let mut command = Command::new("git");
    command.args(["rev-parse", "--show-toplevel"]);
    Ok(PathBuf::from(output(&mut command)?.trim_end()))
}

/// The base commit, when it is given and the repository holds it. A Run's
/// checks are told none when GitHub's comparison failed, and a base GitHub
/// gave may not have been fetched.
fn base_commit(cli: &Cli, root: &Path) -> Option<String> {
    let base = cli
        .base
        .clone()
        .or_else(|| std::env::var_os(BASE_VARIABLE).and_then(|value| value.into_string().ok()))
        .filter(|base| !base.is_empty())?;
    let held = git(root)
        .args(["cat-file", "-e", &format!("{base}^{{commit}}")])
        .output()
        .is_ok_and(|output| output.status.success());
    held.then_some(base)
}

/// Fails at once, before any build, when a tool the check runs is missing.
fn require_tools(root: &Path) -> Result<(), Report> {
    for tool in TOOLS {
        output(cargo(root).args([tool, "--version"]))
            .map_err(|_| report!("cargo {tool} is missing: {INSTALL_HINT}"))?;
    }
    Ok(())
}

/// What the changed-paths file says; incomplete when there is none.
fn listing() -> Listing {
    std::env::var_os(CHANGED_VARIABLE)
        .and_then(|path| fs::read_to_string(path).ok())
        .map_or(Listing::Incomplete, |text| changed::read_listing(&text))
}

/// The scored files the change left in the tree, with their diffs and their
/// functions, and those it removed.
fn touched_files(root: &Path, base: &str, listing: Listing) -> Result<Touched, Report> {
    let pathspec: Vec<String> = match listing {
        Listing::Complete(paths) => paths
            .into_iter()
            .filter(|path| changed::is_scored(path))
            .collect(),
        Listing::Incomplete => vec!["*.rs".to_owned()],
    };
    if pathspec.is_empty() {
        return Ok(Touched::default());
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

    let diff = diff::parse(&text);
    let mut touched = Touched {
        removed: diff
            .removed
            .into_iter()
            .filter(|path| changed::is_scored(path))
            .collect(),
        ..Touched::default()
    };
    for file in diff
        .files
        .into_iter()
        .filter(|file| changed::is_scored(&file.path))
    {
        touched_file(root, file, &mut touched)?;
    }
    Ok(touched)
}

/// Adds the file with its functions, unless it is generated code; one that
/// does not parse gets a warning instead.
fn touched_file(root: &Path, diff: FileDiff, touched: &mut Touched) -> Result<(), Report> {
    let source = fs::read_to_string(root.join(&diff.path))
        .context(format!("could not read {}", diff.path))?;
    if changed::is_generated(&source) {
        return Ok(());
    }

    match spans::functions(&source) {
        Ok(functions) => touched.files.push(TouchedFile { diff, functions }),
        Err(error) => touched.warnings.push(Warning {
            path: diff.path,
            warning: format!("not scored: it does not parse ({error})"),
        }),
    }
    Ok(())
}

/// The scores of every function in the packages holding the touched files,
/// against those packages' own tests. A file no package holds gets a
/// warning and is dropped.
fn score_change(root: &Path, touched: &mut Touched, work: &Path) -> Result<Scored, Report> {
    let metadata: Metadata = serde_json::from_str(&output(cargo(root).args([
        "metadata",
        "--format-version",
        "1",
        "--no-deps",
    ]))?)
    .context("could not read cargo metadata")?;
    let paths: Vec<String> = touched
        .files
        .iter()
        .map(|file| file.diff.path.clone())
        .collect();
    for path in metadata.outside(&paths) {
        touched.warnings.push(Warning {
            warning: "not scored: no package of the workspace holds it".to_owned(),
            path,
        });
    }

    let packages: Vec<String> = metadata.packages_of(&paths).into_keys().collect();
    if packages.is_empty() {
        return Ok(Scored {
            entries: Vec::new(),
            failed_tests: Vec::new(),
        });
    }
    measure(root, &packages, work)
}

/// Runs the packages' tests under coverage and scores them. Rounds on one
/// machine share the coverage build, one at a time.
fn measure(root: &Path, packages: &[String], work: &Path) -> Result<Scored, Report> {
    let target = coverage_target(root);
    fs::create_dir_all(&target).context("could not create the coverage build's folder")?;
    let lock = File::create(target.join("crap-rust.lock"))
        .context("could not create the coverage build's lock")?;
    lock.lock().context("could not lock the coverage build")?;

    let selection: Vec<&str> = packages
        .iter()
        .flat_map(|name| ["-p", name.as_str()])
        .collect();
    let lcov = work.join("lcov.info");
    let report = work.join("change.json");
    let nextest_config = work.join("nextest.toml");
    fs::write(&nextest_config, NEXTEST_CONFIG).context("could not write nextest's settings")?;

    // Profiles an earlier run left would count as this run's coverage. A
    // failing test is the tests' check's to report: the coverage of the
    // others still scores, and the rows say it may be low.
    run(cargo(root).env("CARGO_LLVM_COV_TARGET_DIR", &target).args([
        "llvm-cov",
        "clean",
        "--profraw-only",
    ]))?;
    let tests = logged_output(
        cargo(root)
            .env("CARGO_LLVM_COV_TARGET_DIR", &target)
            .args(["llvm-cov", "nextest", "--no-clean", "--ignore-run-fail"])
            .args(&selection)
            .args(["--profile", NEXTEST_PROFILE, "--tool-config-file"])
            .arg(tool_config(&nextest_config))
            .arg("--lcov")
            .arg("--output-path")
            .arg(&lcov),
    )?;
    run(cargo(root)
        .args(["crap", "--format", "json"])
        .args(&selection)
        .args(TEST_FILES)
        .arg("--lcov")
        .arg(&lcov)
        .arg("--output")
        .arg(&report))?;

    Ok(Scored {
        entries: read_scores(&report)?,
        failed_tests: nextest::failed_tests(&tests),
    })
}

/// `crap-rust:<path>`, as nextest takes a tool's settings.
fn tool_config(path: &Path) -> OsString {
    let mut argument = OsString::from("crap-rust:");
    argument.push(path);
    argument
}

/// Where the coverage build goes: `CARGO_LLVM_COV_TARGET_DIR`, else
/// cargo-llvm-cov's own folder in `CARGO_TARGET_DIR`, else the user's cache.
fn coverage_target(root: &Path) -> PathBuf {
    let variable = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(target) = variable("CARGO_LLVM_COV_TARGET_DIR") {
        return root.join(target);
    }
    if let Some(target) = variable("CARGO_TARGET_DIR") {
        return root.join(target).join("llvm-cov-target");
    }

    let cache = variable("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| variable("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| root.join("target"));
    cache.join(COVERAGE_TARGET)
}

/// The base versions of the touched files and of the removed ones, written
/// under `tree` at their paths, with their functions.
fn base_files(
    root: &Path,
    base: &str,
    touched: &Touched,
    tree: &Path,
) -> Result<Vec<BaseFile>, Report> {
    let changed = touched
        .files
        .iter()
        .filter_map(|file| file.diff.base_path.as_deref());
    let mut files = Vec::new();
    for path in changed.chain(touched.removed.iter().map(String::as_str)) {
        let source = output(git(root).arg("show").arg(format!("{base}:{path}")))?;
        let copy = tree.join(path);
        fs::create_dir_all(copy.parent().unwrap_or(tree))
            .context(format!("could not copy {path}"))?;
        fs::write(&copy, &source).context(format!("could not copy {path}"))?;

        // A base version that does not parse still has its complexities
        // scored; only a moved function is not found in it.
        let functions = spans::functions(&source).unwrap_or_default();
        files.push(BaseFile {
            path: path.to_owned(),
            functions,
        });
    }

    // `cargo crap` reads its settings from the folder it runs in, so the
    // base's scores count complexity as the change's do.
    if root.join(CONFIG).is_file() && !files.is_empty() {
        fs::copy(root.join(CONFIG), tree.join(CONFIG))
            .context(format!("could not copy {CONFIG}"))?;
    }
    Ok(files)
}

/// The complexities of every function in the base files under `tree`.
fn score_base(files: &[BaseFile], tree: &Path) -> Result<Vec<Entry>, Report> {
    if files.is_empty() {
        return Ok(Vec::new());
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
    #[serde(default = "default_threshold")]
    threshold: f64,
}

fn default_threshold() -> f64 {
    DEFAULT_THRESHOLD
}

/// The score above which a function fails, from `.cargo-crap.toml`.
fn threshold(root: &Path) -> Result<f64, Report> {
    let Ok(text) = fs::read_to_string(root.join(CONFIG)) else {
        return Ok(DEFAULT_THRESHOLD);
    };
    let config: Config = toml::from_str(&text).context(format!("could not read {CONFIG}"))?;
    Ok(config.threshold)
}

/// How many failed tests the summary names.
const NAMED_FAILURES: usize = 3;

/// Prints the warnings, the rows and a summary; true when a row fails.
fn print(
    warnings: &[Warning],
    rows: &[Row],
    failed_tests: &[String],
    threshold: f64,
) -> Result<bool, Report> {
    for warning in warnings {
        println!(
            "{}",
            serde_json::to_string(warning).context("could not write a warning")?
        );
    }
    for row in rows {
        println!(
            "{}",
            serde_json::to_string(row).context("could not write a row")?
        );
    }

    let failing = rows.iter().filter(|row| row.fails).count();
    let unproven = rows.iter().filter(|row| row.unproven).count();
    let others = rows
        .iter()
        .filter(|row| !row.fails && !row.unproven && row.score > threshold)
        .count();
    println!(
        "crap-rust: {} touched functions; {failing} new or more complex above {threshold} (failing); {unproven} unproven; {others} others above it{}",
        rows.len(),
        failures(failed_tests),
    );
    Ok(failing > 0)
}

/// The summary's note of failed tests, naming the first few.
fn failures(failed_tests: &[String]) -> String {
    if failed_tests.is_empty() {
        return String::new();
    }

    let named: Vec<&str> = failed_tests
        .iter()
        .take(NAMED_FAILURES)
        .map(String::as_str)
        .collect();
    format!(
        "; {} tests failed ({}), so coverage may be low",
        failed_tests.len(),
        named.join(", ")
    )
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

/// Runs `command` and returns all it wrote, after copying it to this
/// command's error stream.
fn logged_output(command: &mut Command) -> Result<String, Report> {
    let output = command
        .output()
        .context(format!("could not run {command:?}"))?;
    let text = [output.stdout.as_slice(), output.stderr.as_slice()]
        .map(String::from_utf8_lossy)
        .concat();
    eprint!("{text}");
    if !output.status.success() {
        bail!("{command:?} failed with {}", output.status);
    }
    Ok(text)
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
