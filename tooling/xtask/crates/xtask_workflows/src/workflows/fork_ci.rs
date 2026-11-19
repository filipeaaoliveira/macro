//! `Fork CI` — Rust and web checks for pull requests in a fork of this
//! repository. Generated into `fork-ci.yml`.
//!
//! A fork has no Namespace runners, so the checks of `PR CI` queue there
//! forever. This workflow runs a smaller set of the same checks, with the same
//! scripts, path filters and `just` recipes, on a self-hosted runner labelled
//! [`RUNNER`]: fmt, unused dependencies, clippy as the offline SQLx check, and
//! cargo nextest against Postgres and Redis for the Rust packages a pull
//! request affects; Biome, the app's types and the affected Vitest tests for
//! the web app. `Fork CI gate` is the one check a branch rule requires.
//!
//! Only branches pushed to the fork itself reach the self-hosted runner, and a
//! pull request from another repository fails the gate. That condition is not
//! the protection: such a pull request runs its own copy of this workflow, with
//! the condition removed if it likes. The protection is the fork's Actions
//! setting "Require approval for all external contributors", which must stay on,
//! and never approving a run from another repository's pull request. A pull
//! request from the upstream repository counts as one, so an upstream sync
//! goes through a branch of the fork instead.

use gh_workflow::{
    Concurrency, Event, Expression, Job, Level, Permissions, PullRequest, PullRequestType, Run,
    Step, Use, Workflow, WorkflowDispatch,
};
use xtask_paths::RuntimePath;

use crate::workflows::{
    code_check_cloud_storage,
    steps::{self, FluentBuilder},
    vars, web_app_check_main,
};

#[cfg(test)]
mod test;

/// The self-hosted runner scale set's name, which is its `runs-on` label.
const RUNNER: &str = "macro-fork";

/// GitHub-hosted runner, free on a public repository, for the light jobs. A
/// pull request from another repository runs only there.
const HOSTED_RUNNER: &str = "ubuntu-latest";

/// The name of the job a branch rule requires.
pub const GATE_NAME: &str = "Fork CI gate";

/// A manual run, or a pull request from a branch of this repository. Jobs on
/// [`RUNNER`] run only then. This only keeps this repository's copy of the
/// workflow from running another repository's code; see the module docs for
/// what protects the runner.
const TRUSTED: &str = "github.event_name == 'workflow_dispatch' || \
     github.event.pull_request.head.repo.full_name == github.repository";

/// Where `changes` builds the `xtask_nextest_filter` binary.
const FILTER_CACHE_DIR: RuntimePath<'static> =
    xtask_paths::runtime_path!("/home/runner/.cache/macro-nextest-filter");

/// Nextest's threads on the self-hosted runner, which has 4 CPUs.
const NEXTEST_TEST_THREADS: u32 = 8;

/// Build the workflow.
pub fn fork_ci() -> Workflow {
    Workflow::new("Fork CI")
        .on(Event::default()
            .pull_request(
                PullRequest::default()
                    .add_branch("main")
                    .add_branch("feat/factory")
                    .add_type(PullRequestType::Opened)
                    .add_type(PullRequestType::Synchronize)
                    .add_type(PullRequestType::Reopened)
                    .add_type(PullRequestType::ReadyForReview),
            )
            .workflow_dispatch(WorkflowDispatch::default()))
        .permissions(Permissions::default().contents(Level::Read))
        .concurrency(
            Concurrency::new(Expression::new(
                "fork-ci-${{ github.event.pull_request.number || github.ref }}",
            ))
            .cancel_in_progress(true),
        )
        .map(vars::with_global_env)
        .add_job("changes", changes())
        .add_job("rust", rust())
        .add_job("web", web())
        .add_job("gate", gate())
}

/// Decide which checks run, and the Rust packages they cover: the same filters
/// and scripts as `cloud storage code check` and `Web App Pr Checks`. Builds
/// the package filter from the pull request's code, so it runs on a hosted
/// runner, and only for a trusted pull request. A manual run skips the filters
/// and checks the whole branch: every package, every web file.
fn changes() -> Job {
    let rust = "github.event_name == 'workflow_dispatch' \
         || steps.rust-filter.outputs.should_run == 'true' \
         || steps.fork-filter.outputs.should_run == 'true'";
    let web = "github.event_name == 'workflow_dispatch' \
         || steps.web-filter.outputs.should_run == 'true' \
         || steps.fork-filter.outputs.should_run == 'true'";
    let scope_rust = format!(
        "github.event_name == 'pull_request' && ({TRUSTED}) \
         && (steps.rust-filter.outputs.should_run == 'true' \
         || steps.fork-filter.outputs.should_run == 'true')"
    );

    Job::default()
        .timeout_minutes(20u32)
        .runs_on(HOSTED_RUNNER)
        // The path filters read the pull request's files from the API.
        .permissions(
            Permissions::default()
                .contents(Level::Read)
                .pull_requests(Level::Read),
        )
        .add_output("rust", format!("${{{{ {rust} }}}}"))
        .add_output("web", format!("${{{{ {web} }}}}"))
        .add_output(
            "rust_packages",
            "${{ github.event_name == 'workflow_dispatch' && 'all' \
             || steps.nextest-filter.outputs.rust_packages }}",
        )
        .add_output(
            "skip_tests",
            "${{ github.event_name == 'pull_request' \
             && steps.nextest-filter.outputs.skip_tests == 'true' }}",
        )
        // PR merge commits need both parents for `git merge-base`.
        .add_step(steps::checkout(false, false).add_with(("fetch-depth", 2)))
        .add_step(on_pull_request(
            code_check_cloud_storage::paths_filter().id("rust-filter"),
        ))
        .add_step(on_pull_request(
            web_app_check_main::paths_filter().id("web-filter"),
        ))
        .add_step(on_pull_request(fork_paths_filter()))
        .add_step(steps::setup_rust_light().if_condition(Expression::new(scope_rust.clone())))
        .add_step(compute_changed_files().if_condition(Expression::new(scope_rust.clone())))
        .add_step(compute_nextest_filter().if_condition(Expression::new(scope_rust)))
}

/// fmt, unused dependencies, clippy and the tests of the affected packages.
/// Clippy runs with `SQLX_OFFLINE=true`, so a missing or stale `.sqlx` entry
/// fails it. The tests run every migration on an empty Postgres first.
fn rust() -> Job {
    let tests = "needs.changes.outputs.rust_packages != 'none' \
         && needs.changes.outputs.skip_tests != 'true'";

    self_hosted_job("rust")
        .timeout_minutes(90u32)
        .add_env((
            "RUST_PACKAGES",
            "${{ needs.changes.outputs.rust_packages }}",
        ))
        .add_env(("NEXTEST_TEST_THREADS", NEXTEST_TEST_THREADS))
        .add_env(("REDIS_URI", "redis://127.0.0.1:6379"))
        .map(code_check_cloud_storage::with_test_compile_env)
        // The default 10-minute idle timeout can stop the server during long
        // link or test phases, which resets the stats reported at the end.
        .add_env(("SCCACHE_IDLE_TIMEOUT", "0"))
        .add_service("postgres", code_check_cloud_storage::postgres_service())
        .add_service("redis", code_check_cloud_storage::redis_service())
        .add_step(steps::checkout(false, false).add_with(("fetch-depth", 2)))
        // Before the dev shell, as in `web`.
        .add_step(changed_files_on_pull_request())
        .add_step(steps::setup_nix())
        .add_step(steps::setup_dev_shell().timeout_minutes(vars::DEV_SHELL_TIMEOUT_MINUTES))
        .add_step(use_runner_cache())
        .add_step(steps::start_sccache_server())
        .add_step(steps::code_mode_assets())
        .add_step(cargo_fmt())
        .add_step(Step::new("unused dependencies").run("cargo shear --format github"))
        .add_step(cargo_clippy())
        .add_step(
            code_check_cloud_storage::configure_postgres().if_condition(Expression::new(tests)),
        )
        .add_step(prepare_tests().if_condition(Expression::new(tests)))
        .add_step(run_tests().if_condition(Expression::new(tests)))
        .add_step(steps::show_sccache_stats())
        .add_step(steps::teardown_nix())
}

/// Biome on the changed files, the app's types, and the Vitest tests the
/// changed files reach.
fn web() -> Job {
    self_hosted_job("web")
        .timeout_minutes(45u32)
        // Full history, as `Web App Pr Checks`' Typecheck: the entity icon check
        // reads every committed list of icon names. Without blobs, which git
        // fetches as it reads them.
        .add_step(
            steps::checkout(false, false)
                .add_with(("fetch-depth", 0))
                .add_with(("filter", "blob:none")),
        )
        // Before the dev shell: its LD_LIBRARY_PATH points the runner's
        // git-remote-https at Nix's glibc, which aborts `git fetch`.
        .add_step(changed_files_on_pull_request())
        .add_step(steps::setup_nix())
        .add_step(
            steps::setup_reqs_web("Setup Prereqs", false)
                .timeout_minutes(vars::DEV_SHELL_TIMEOUT_MINUTES),
        )
        .add_step(use_runner_cache())
        .add_step(biome("Run Biome", xtask_paths::repo_dir!("apps/web")))
        .add_step(biome(
            "Run Collaboration Package Biome",
            xtask_paths::repo_dir!("packages/collaboration"),
        ))
        // The TypeScript 7 compiler is a Go program, and Go's collector lets the
        // heap grow to twice what is live unless told a limit. Without one, and
        // with fewer checkers too, it held the runner's whole 9Gi and stalled for
        // twenty minutes.
        .add_step(
            Step::new("Check Types")
                .run("bun run type-check --checkers 2")
                .add_env(("GOMEMLIMIT", "6GiB"))
                .working_directory(xtask_paths::repo_dir!("apps/web")),
        )
        .add_step(
            Step::new("Build WASM for tests")
                // The workflow's RUSTFLAGS name a host linker, which the wasm
                // linker rejects. Unset, cargo reads the wasm target's own flags
                // from .cargo/config.toml, as `Web App Pr Checks` builds them.
                .run("unset RUSTFLAGS\njust build-agent-fold-wasm\njust build-natural-time-wasm")
                .working_directory(xtask_paths::repo_dir!("apps/web"))
                .timeout_minutes(15u32),
        )
        .add_step(vitest())
        .add_step(steps::teardown_nix())
}

/// Passes when every check that had something to check passed. A job skipped
/// because its filter matched nothing counts as passed. Runs on a hosted runner
/// and on every run, so no condition can skip the required check.
fn gate() -> Job {
    Job::default()
        .name(GATE_NAME)
        .needs(vec![
            "changes".to_string(),
            "rust".to_string(),
            "web".to_string(),
        ])
        .cond(Expression::new("always()"))
        .runs_on(HOSTED_RUNNER)
        .timeout_minutes(5u32)
        .add_step(
            Step::new("Check the results")
                .run(GATE_SCRIPT)
                .shell("bash")
                .add_env(("NEEDS", "${{ toJSON(needs) }}"))
                .add_env(("TRUSTED", format!("${{{{ {TRUSTED} }}}}"))),
        )
}

const GATE_SCRIPT: &str = indoc::indoc! {r#"
    set -euo pipefail

    if [ "$TRUSTED" != true ]; then
      echo "::error::Fork CI checks only branches of this repository. Push the branch here and open the pull request from it."
      exit 1
    fi

    failed=false
    while read -r job result; do
      echo "$job: $result"
      if [ "$job" = changes ] && [ "$result" != success ]; then
        failed=true
      fi
      if [ "$result" != success ] && [ "$result" != skipped ]; then
        failed=true
      fi
    done < <(jq -r 'to_entries[] | "\(.key) \(.value.result)"' <<< "$NEEDS")

    if [ "$failed" = true ]; then
      echo "::error::A Fork CI check failed or was cancelled."
      exit 1
    fi
"#};

/// A job on the self-hosted runner, after `changes`, when its filter matched
/// and the run is trusted.
fn self_hosted_job(filter_output: &str) -> Job {
    Job::default()
        .needs(vec!["changes".to_string()])
        .cond(Expression::new(format!(
            "needs.changes.outputs.{filter_output} == 'true' && ({TRUSTED})"
        )))
        .runs_on(RUNNER)
}

fn on_pull_request(step: Step<Use>) -> Step<Use> {
    step.if_condition(Expression::new("github.event_name == 'pull_request'"))
}

/// This workflow and its source: a change to either runs both checks.
fn fork_paths_filter() -> Step<Use> {
    Step::new("Filter changed paths of this workflow")
        .uses(
            "dorny",
            "paths-filter",
            "d1c1ffe0248fe513906c8e24db8ea791d46f8590",
        ) // v3.0.3
        .id("fork-filter")
        .add_with((
            "filters",
            indoc::indoc! {r#"
                should_run:
                  - .github/workflows/fork-ci.yml
                  - 'tooling/xtask/crates/xtask_workflows/src/workflows/fork_ci.rs'
                  - 'tooling/xtask/crates/xtask_workflows/src/workflows/fork_ci/**'
            "#},
        ))
}

/// Writes the changed files to `/tmp/changed-files` and the revision they are
/// compared with to `/tmp/base-revision`.
fn compute_changed_files() -> Step<Run> {
    Step::new("compute changed files")
        .run(include_str!("scripts/compute_changed_files.sh"))
        .shell("bash")
}

fn compute_nextest_filter() -> Step<Run> {
    Step::new("compute nextest package filter")
        .run(include_str!("scripts/compute_nextest_filter.sh"))
        .id("nextest-filter")
        .add_env(("FILTER_CACHE_DIR", FILTER_CACHE_DIR.as_str()))
        .shell("bash")
}

/// [`compute_changed_files`] for a pull request. A manual run has no base
/// revision, so its checks cover every file.
fn changed_files_on_pull_request() -> Step<Run> {
    compute_changed_files().if_condition(Expression::new("github.event_name == 'pull_request'"))
}

/// `cargo fmt --check` on the packages that own a changed Rust file, so a file
/// this repository left unformatted fails only the pull request that changes
/// its package. A manual run checks the whole workspace, as upstream does.
fn cargo_fmt() -> Step<Run> {
    Step::new("fmt").run(indoc::indoc! {r#"
        set -euo pipefail

        if [ ! -f /tmp/base-revision ]; then
          cargo fmt --check
          exit 0
        fi

        # The package of each changed Rust file: the name in the nearest
        # manifest above it that has a [package] table.
        packages=()
        while read -r file; do
          [ -f "$file" ] || continue
          dir=$(dirname "$file")
          while [ "$dir" != . ]; do
            name=$(sed -n '/^\[package\]/,/^\[/s/^name *= *"\(.*\)"/\1/p' "$dir/Cargo.toml" 2>/dev/null || true)
            if [ -n "$name" ]; then
              packages+=("$name")
              break
            fi
            dir=$(dirname "$dir")
          done
        done < <(grep -E '\.rs$' /tmp/changed-files || true)

        if [ "${#packages[@]}" -eq 0 ]; then
          echo "No changed Rust files"
          exit 0
        fi

        args=()
        for name in $(printf '%s\n' "${packages[@]}" | sort -u); do
          args+=(-p "$name")
        done
        echo "fmt: ${args[*]}"
        cargo fmt --check "${args[@]}"
    "#})
}

/// Keep build output in the runner's cache directory when it has one
/// (`CI_CACHE_DIR`), so the next job on it starts warm. Clippy, the tests and
/// the web job build with different flags, so each has its own target
/// directory: sharing one would rebuild everything each time.
fn use_runner_cache() -> Step<Run> {
    Step::new("Use the runner's cache").run(indoc::indoc! {r#"
        set -euo pipefail
        cache="${CI_CACHE_DIR:-$RUNNER_TEMP/cache}"
        echo "Caching builds in $cache"
        {
          echo "CARGO_TARGET_ROOT=$cache/target"
          echo "WASM_PKG_CACHE_DIR=$cache/wasm-pkgs"
          echo "VITEST_MODULE_CACHE_DIR=$cache/vitest"
        } >> "$GITHUB_ENV"
    "#})
}

/// The clippy flags of `cloud storage code check`, scoped to the same packages.
fn cargo_clippy() -> Step<Run> {
    Step::new("clippy")
        .run(format!(
            "export CARGO_TARGET_DIR=\"$CARGO_TARGET_ROOT/clippy\"\n{}",
            include_str!("scripts/cargo_clippy.sh")
        ))
        .add_env((
            "RUSTFLAGS",
            "-Dwarnings -Dclippy::disallowed_methods -C link-arg=-fuse-ld=mold",
        ))
        .add_env(("RUSTDOCFLAGS", "-Dwarnings"))
        .add_env(("SQLX_OFFLINE", "true"))
}

/// Set up test env files and databases: every migration runs on the empty
/// `macrodb`, then on `template1`, which each `#[sqlx::test]` database copies.
fn prepare_tests() -> Step<Run> {
    Step::new("prepare tests")
        .run("just setup_test_envs && just initialize_dbs && just setup_test_template")
}

fn run_tests() -> Step<Run> {
    Step::new("run tests").run(format!(
        "export CARGO_TARGET_DIR=\"$CARGO_TARGET_ROOT/test\"\n{}",
        include_str!("scripts/run_tests.sh")
    ))
}

/// Biome on the files changed since the base revision, as `Web App Pr Checks`
/// runs it against `main`. Without a base revision (a manual run, or no
/// merge-base turned up), it checks every file.
fn biome(name: &str, dir: xtask_paths::RepoDir<'_>) -> Step<Run> {
    Step::new(name)
        .run(indoc::indoc! {r#"
            set -euo pipefail
            changed=()
            if [ -f /tmp/base-revision ]; then
              changed=(--changed --since="$(cat /tmp/base-revision)")
            fi
            biome ci "${changed[@]}" --no-errors-on-unmatched --error-on-warnings
        "#})
        .working_directory(dir)
}

/// The tests that import a changed file, directly or not. Without a base
/// revision (a manual run, or no merge-base turned up), every test.
fn vitest() -> Step<Run> {
    Step::new("Test")
        .run(indoc::indoc! {r#"
            set -euo pipefail
            changed=()
            if [ -f /tmp/base-revision ]; then
              changed=(--changed "$(cat /tmp/base-revision)" --passWithNoTests)
            fi
            bun run test "${changed[@]}"
        "#})
        .working_directory(xtask_paths::repo_dir!("apps/web"))
        // Node's default heap on this runner is about 2 GB, and Vitest's main
        // process ran out of it walking the app's module graph.
        .add_env(("NODE_OPTIONS", "--max-old-space-size=6144"))
}
