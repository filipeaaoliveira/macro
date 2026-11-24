//! `Fork CI` — Rust and web checks for pull requests in a fork of this
//! repository. Generated into `fork-ci.yml`.
//!
//! A fork has no Namespace runners, so the checks of `PR CI` queue there
//! forever. This workflow runs a smaller set of the same checks, with the same
//! scripts, path filters and `just` recipes, on GitHub-hosted runners, free on
//! a public repository: fmt, unused dependencies, clippy as the offline SQLx
//! check, and cargo nextest against Postgres and Redis for the Rust packages a
//! pull request affects, split across [`MAX_TEST_SHARDS`] jobs at most; and
//! Biome, the app's types and the affected Vitest tests for the web app.
//! `Fork CI gate` is the one check a branch rule requires.
//!
//! A hosted runner keeps nothing between jobs, so the Rust jobs save their
//! compiled dependencies and the code-mode assets to the Actions cache. A
//! pull request reads the entries its own earlier runs saved, and those of its
//! base branch.
//!
//! The Rust jobs run only for a manual run or a branch pushed to the fork
//! itself, and a pull request from another repository fails the gate. That
//! condition is not the protection: such a pull request runs its own copy of
//! this workflow, with the condition removed if it likes. The protection is the
//! fork's Actions setting "Require approval for all external contributors",
//! which must stay on, and never approving a run from another repository's pull
//! request. A pull request from the upstream repository counts as one, so an
//! upstream sync goes through a branch of the fork instead.

use gh_workflow::{
    Concurrency, Event, Expression, Job, Level, Permissions, PullRequest, PullRequestType, Run,
    Step, Strategy, Use, Workflow, WorkflowDispatch,
};
use xtask_paths::RuntimePath;

use crate::workflows::{
    code_check_cloud_storage,
    steps::{self, FluentBuilder},
    vars, web_app_check_main,
};

#[cfg(test)]
mod test;

/// GitHub-hosted runner, free on a public repository: 4 CPUs and 16 GB. Its
/// one disk had 86 GB free before [`free_disk_space`] and 117 GB after.
const HOSTED_RUNNER: &str = "ubuntu-latest";

/// The name of the job a branch rule requires.
pub const GATE_NAME: &str = "Fork CI gate";

/// A manual run, or a pull request from a branch of this repository. The Rust
/// jobs run only then. This only keeps this repository's copy of the workflow
/// from running another repository's code; see the module docs for what
/// protects it.
const TRUSTED: &str = "github.event_name == 'workflow_dispatch' || \
     github.event.pull_request.head.repo.full_name == github.repository";

/// Where `changes` builds the `xtask_nextest_filter` binary.
const FILTER_CACHE_DIR: RuntimePath<'static> =
    xtask_paths::runtime_path!("/home/runner/.cache/macro-nextest-filter");

/// The Rust jobs' build output and caches, relative to the checkout: under
/// `target/`, which git and the Rust tools ignore, and inside the workspace,
/// where `rust-cache` looks for target directories.
const BUILD_DIR: &str = "target/fork-ci";

/// Nextest's threads in a test shard. More than its 4 CPUs: most tests wait
/// on Postgres, in a service container on the same runner.
const NEXTEST_TEST_THREADS: u32 = 8;

/// Cargo's jobs in a test shard. One compiler of a large crate alone held
/// 4.4 GiB, so three fit in the runner's 16 GB with room for cargo and one
/// link.
const TEST_BUILD_JOBS: u32 = 3;

/// Cargo's jobs in the clippy job: all 4 CPUs. Clippy generates no code and
/// holds less than a build.
const CHECK_BUILD_JOBS: u32 = 4;

/// The most test shards a run splits the affected packages across. Each
/// shard builds and tests its own share of the packages on its own runner.
const MAX_TEST_SHARDS: u32 = 4;

/// Affected packages per test shard, until [`MAX_TEST_SHARDS`].
const PACKAGES_PER_SHARD: u32 = 4;

/// `Swatinem/rust-cache`, which saves the compiled dependencies of a target
/// directory and cargo's registry.
const RUST_CACHE_VERSION: &str = "v2.9.3";

/// actions/cache v5.0.5, as `build_web_app` pins it.
const ACTIONS_CACHE_SHA: &str = "27d5ce7f107fe9357f9df03efb73ab90386fccae";

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
        .add_job("rust-check", rust_check())
        .add_job("rust-test", rust_test())
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
        .add_step(test_shards())
        .add_output("test_shards", "${{ steps.test-shards.outputs.shards }}")
        .add_output("test_shard_count", "${{ steps.test-shards.outputs.count }}")
}

/// How many test shards the affected packages need: one per
/// [`PACKAGES_PER_SHARD`] packages, at most [`MAX_TEST_SHARDS`], and the most
/// for every package. `sync_service` has no tests here, so it doesn't count.
/// Writes `count` and `shards`, the matrix of shard numbers. It reads only
/// the other steps' outputs, never the pull request's code.
fn test_shards() -> Step<Run> {
    Step::new("count the test shards")
        .run(indoc::indoc! {r#"
            set -euo pipefail
            if [ "$RUST_PACKAGES" = all ]; then
              count=$MAX_TEST_SHARDS
            else
              packages=$(tr ' ' '\n' <<< "$RUST_PACKAGES" | grep -cvE '^(sync_service|none|)$' || true)
              count=$(( (packages + PACKAGES_PER_SHARD - 1) / PACKAGES_PER_SHARD ))
              count=$(( count < 1 ? 1 : count > MAX_TEST_SHARDS ? MAX_TEST_SHARDS : count ))
            fi
            echo "count=$count" >> "$GITHUB_OUTPUT"
            echo "shards=$(seq -s , 1 "$count" | sed 's/.*/[&]/')" >> "$GITHUB_OUTPUT"
            echo "$count test shards"
        "#})
        .id("test-shards")
        .shell("bash")
        .add_env((
            "RUST_PACKAGES",
            "${{ github.event_name == 'workflow_dispatch' && 'all' \
             || steps.nextest-filter.outputs.rust_packages }}",
        ))
        .add_env(("MAX_TEST_SHARDS", MAX_TEST_SHARDS))
        .add_env(("PACKAGES_PER_SHARD", PACKAGES_PER_SHARD))
}

/// fmt, unused dependencies and clippy on the affected packages. Clippy runs
/// with `SQLX_OFFLINE=true`, so a missing or stale `.sqlx` entry fails it. Its
/// own job, beside the test shards, so a failure here shows without waiting
/// for the tests.
fn rust_check() -> Job {
    hosted_rust_job(filtered_job("rust"))
        .timeout_minutes(120u32)
        .add_env(("CARGO_BUILD_JOBS", CHECK_BUILD_JOBS))
        .map(code_check_cloud_storage::with_test_compile_env)
        .add_step(steps::checkout(false, false).add_with(("fetch-depth", 2)))
        // Before the dev shell, as in `web`.
        .add_step(changed_files_on_pull_request())
        .add_step(free_disk_space())
        .add_step(steps::setup_nix())
        .add_step(steps::setup_dev_shell().timeout_minutes(vars::DEV_SHELL_TIMEOUT_MINUTES))
        .add_step(use_runner_cache())
        .add_step(without_sccache())
        .add_step(rust_cache("clippy", "clippy"))
        .add_step(code_mode_assets_key())
        .add_step(cache_code_mode_assets())
        .add_step(import_code_mode_assets())
        .add_step(steps::code_mode_assets())
        .add_step(export_code_mode_assets())
        .add_step(cargo_fmt())
        .add_step(Step::new("unused dependencies").run("cargo shear --format github"))
        .add_step(cargo_clippy().add_env(rust_packages()))
        .add_step(show_disk_usage())
}

/// The tests of the affected packages, after every migration runs on an empty
/// Postgres, split across the shards `changes` counted: shard `k` of `n` takes
/// every `n`th package from the `k`th, so each builds only its share. Skipped
/// when no package is affected, or only `.sqlx` changed. A change to a widely
/// used crate selects dozens of packages, which take a shard's 4 CPUs over an
/// hour to build and test cold, hence the timeout.
fn rust_test() -> Job {
    let job = Job::default()
        .needs(vec!["changes".to_string()])
        .cond(Expression::new(format!(
            "needs.changes.outputs.rust == 'true' \
             && needs.changes.outputs.rust_packages != 'none' \
             && needs.changes.outputs.skip_tests != 'true' && ({TRUSTED})"
        )));
    hosted_rust_job(job)
        .strategy(Strategy {
            fail_fast: Some(false),
            matrix: Some(serde_json::json!({
                "shard": "${{ fromJSON(needs.changes.outputs.test_shards) }}",
            })),
            max_parallel: None,
        })
        .timeout_minutes(180u32)
        .add_env(("TEST_SHARD", "${{ matrix.shard }}"))
        .add_env((
            "TEST_SHARD_COUNT",
            "${{ needs.changes.outputs.test_shard_count }}",
        ))
        .add_env(("NEXTEST_TEST_THREADS", NEXTEST_TEST_THREADS))
        // Unbounded, rustc and linker processes on the large service crates
        // fill the runner's memory, and the job pages instead of building.
        .add_env(("CARGO_BUILD_JOBS", TEST_BUILD_JOBS))
        .add_env(("MOLD_JOBS", 1u32))
        .add_env(("REDIS_URI", "redis://127.0.0.1:6379"))
        .map(code_check_cloud_storage::with_test_compile_env)
        .add_service("postgres", code_check_cloud_storage::postgres_service())
        .add_service("redis", code_check_cloud_storage::redis_service())
        .add_step(steps::checkout(false, false))
        .add_step(free_disk_space())
        .add_step(steps::setup_nix())
        .add_step(steps::setup_dev_shell().timeout_minutes(vars::DEV_SHELL_TIMEOUT_MINUTES))
        .add_step(use_runner_cache())
        .add_step(without_sccache())
        .add_step(rust_cache("test", "test-${{ matrix.shard }}"))
        .add_step(code_mode_assets_key())
        .add_step(cache_code_mode_assets())
        .add_step(import_code_mode_assets())
        .add_step(steps::code_mode_assets())
        .add_step(export_code_mode_assets())
        .add_step(code_check_cloud_storage::configure_postgres())
        .add_step(prepare_tests())
        .add_step(run_tests().add_env(rust_packages()))
        .add_step(show_disk_usage())
}

/// The packages `changes` selected, for the one step that reads them.
/// `rust-cache` keys its entries on every `RUST*` variable it sees, so a
/// job-wide `RUST_PACKAGES` would give each set of packages its own entries,
/// and a pull request would never restore another's.
fn rust_packages() -> (&'static str, &'static str) {
    (
        "RUST_PACKAGES",
        "${{ needs.changes.outputs.rust_packages }}",
    )
}

/// A Rust job on a hosted runner, building in [`BUILD_DIR`]. No
/// `teardown_nix`: it frees /nix for a cache volume to unmount, and on a hosted
/// runner, where /nix is on the root file system, its `fuser -km /nix` kills
/// every process on it, the runner's included.
fn hosted_rust_job(job: Job) -> Job {
    job.runs_on(HOSTED_RUNNER).add_env((
        "CI_CACHE_DIR",
        format!("${{{{ github.workspace }}}}/{BUILD_DIR}"),
    ))
}

/// Deletes the hosted image's SDKs and Docker images that no job here uses,
/// in parallel, and creates [`BUILD_DIR`]. Prints the free space before and
/// after.
fn free_disk_space() -> Step<Run> {
    Step::new("free disk space")
        .run(indoc::indoc! {r#"
            set -euo pipefail
            df -h /
            for dir in /usr/share/dotnet /usr/local/lib/android /opt/ghc \
              /usr/local/.ghcup /opt/hostedtoolcache/CodeQL /usr/share/swift \
              /usr/local/share/powershell /usr/local/share/chromium; do
              sudo rm -rf "$dir" &
            done
            # Images a service container uses stay.
            docker image prune --all --force > /dev/null &
            wait
            mkdir -p "$CI_CACHE_DIR"
            df -h /
        "#})
        .shell("bash")
}

/// Builds without the dev shell's sccache. Its local cache starts empty on a
/// hosted runner and would only copy every compiled crate; `rust_cache`
/// restores the compiled dependencies instead.
fn without_sccache() -> Step<Run> {
    Step::new("build without sccache").run("echo 'unset RUSTC_WRAPPER' >> \"$BASH_ENV\"")
}

/// Restores the compiled dependencies of `target`, a directory under
/// `CARGO_TARGET_ROOT`, and cargo's registry, and saves them when the job
/// ends, failed or not: a failed test leaves a good build behind. `key` tells
/// the job's entries from the other jobs'.
fn rust_cache(target: &str, key: &str) -> Step<Use> {
    Step::new("cache compiled dependencies")
        .uses("Swatinem", "rust-cache", RUST_CACHE_VERSION)
        .add_with(("workspaces", format!(". -> {BUILD_DIR}/target/{target}")))
        .add_with(("key", key))
        .add_with(("cache-on-failure", true))
}

/// The store path of the code-mode assets, which names every input of their
/// build, as `code_mode_assets.sh` keys its remote cache.
fn code_mode_assets_key() -> Step<Run> {
    Step::new("code-mode assets key")
        .run(indoc::indoc! {r#"
            set -euo pipefail
            path="$(nix eval --raw .#code-mode-assets.outPath)"
            echo "path=$path" >> "$GITHUB_OUTPUT"
            echo "name=$(basename "$path")" >> "$GITHUB_OUTPUT"
        "#})
        .id("code-mode-assets")
}

/// Under [`BUILD_DIR`], relative to the checkout.
const CODE_MODE_ASSETS_EXPORT: &str = "target/fork-ci/code-mode-assets.closure";

/// Building the code-mode assets installs node_modules and takes minutes, so
/// the job keeps an export of the store path in the Actions cache.
fn cache_code_mode_assets() -> Step<Use> {
    Step::new("cache the code-mode assets")
        .uses("actions", "cache", ACTIONS_CACHE_SHA)
        .id("code-mode-assets-cache")
        .continue_on_error(true)
        .add_with(("path", CODE_MODE_ASSETS_EXPORT))
        .add_with((
            "key",
            "code-mode-assets-${{ steps.code-mode-assets.outputs.name }}",
        ))
}

fn import_code_mode_assets() -> Step<Run> {
    Step::new("import the cached code-mode assets")
        .run(format!(
            "nix-store --import < {CODE_MODE_ASSETS_EXPORT} \
             || echo '::warning::Could not import the cached code-mode assets'"
        ))
        .if_condition(Expression::new(
            "steps.code-mode-assets-cache.outputs.cache-hit == 'true'",
        ))
}

fn export_code_mode_assets() -> Step<Run> {
    Step::new("export the code-mode assets for the cache")
        .run(format!(
            "nix-store --export \"$ASSETS\" > {CODE_MODE_ASSETS_EXPORT}"
        ))
        .add_env(("ASSETS", "${{ steps.code-mode-assets.outputs.path }}"))
        .if_condition(Expression::new(
            "steps.code-mode-assets-cache.outputs.cache-hit != 'true'",
        ))
}

/// The free space left, and what the job's build directories hold.
fn show_disk_usage() -> Step<Run> {
    Step::new("show disk usage")
        .run(indoc::indoc! {r#"
            df -h /
            du -sh /nix/store "$CI_CACHE_DIR"/target/* ~/.cargo/registry 2>/dev/null || true
        "#})
        .if_condition(Expression::new("always()"))
}

/// Biome on the changed files, the app's types, and the Vitest tests the
/// changed files reach. On a GitHub-hosted runner, free on a public
/// repository, beside the Rust jobs: its type check and Vitest hold about
/// 7 GB, which the self-hosted runner's node cannot hold beside a test build,
/// so it would wait for the Rust jobs instead. A hosted runner keeps no
/// caches, so its dev shell, packages and wasm builds start from nothing.
fn web() -> Job {
    filtered_job("web")
        .runs_on(HOSTED_RUNNER)
        .timeout_minutes(60u32)
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
        .add_step(fetch_diff_blobs())
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
        // The last step: no `teardown_nix`. It frees /nix for a cache volume to
        // unmount, and on a hosted runner, where /nix is on the root file system,
        // its `fuser -km /nix` kills every process on it, the runner's included.
        .add_step(vitest())
}

/// Passes when every check that had something to check passed. A job skipped
/// because its filter matched nothing counts as passed. Runs on a hosted runner
/// and on every run, so no condition can skip the required check.
fn gate() -> Job {
    Job::default()
        .name(GATE_NAME)
        .needs(vec![
            "changes".to_string(),
            "rust-check".to_string(),
            "rust-test".to_string(),
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

/// A job after `changes`, when its filter matched and the run is trusted.
fn filtered_job(filter_output: &str) -> Job {
    Job::default()
        .needs(vec!["changes".to_string()])
        .cond(Expression::new(format!(
            "needs.changes.outputs.{filter_output} == 'true' && ({TRUSTED})"
        )))
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

/// The nextest profile of the test job, on top of the repository's default
/// profile. Without a timeout a test that waits forever holds the runner until
/// the job's timeout, and the log of a job that never ends is lost with it.
/// Every failure is reported, so one run shows all of them.
///
/// The email repository's paging tests compare many pages of one query on
/// Postgres, which shares the runner with Redis and the tests: beside the rest
/// of the suite several took between two and five minutes, so they get ten.
const NEXTEST_PROFILE: &str = indoc::indoc! {r#"
    [profile.fork-ci]
    slow-timeout = { period = "60s", terminate-after = 5 }
    leak-timeout = "1s"
    fail-fast = false
    failure-output = "immediate-final"

    [[profile.fork-ci.overrides]]
    filter = "package(email) & test(/^outbound::email_pg_repo::/)"
    slow-timeout = { period = "120s", terminate-after = 5 }
"#};

fn run_tests() -> Step<Run> {
    Step::new("run tests")
        .run(format!(
            "{SHARD_PACKAGES}\
             export CARGO_TARGET_DIR=\"$CARGO_TARGET_ROOT/test\"\n\
             config=\"$RUNNER_TEMP/nextest-fork-ci.toml\"\n\
             cat > \"$config\" <<'EOF'\n{NEXTEST_PROFILE}EOF\n\
             export NEXTEST_TOOL_CONFIG=\"fork-ci:$config\" NEXTEST_PROFILE=fork-ci\n{}",
            include_str!("scripts/run_tests.sh")
        ))
        .timeout_minutes(RUN_TESTS_TIMEOUT_MINUTES)
}

/// Narrows `RUST_PACKAGES` to this shard's share: shard `TEST_SHARD` of
/// `TEST_SHARD_COUNT` takes every `TEST_SHARD_COUNT`th package, from the
/// `TEST_SHARD`th. `all` stands for every package of the workspace but
/// `sync_service`, which `run_tests.sh` leaves out of `all` too.
const SHARD_PACKAGES: &str = indoc::indoc! {r#"
    if [ "$RUST_PACKAGES" = all ]; then
      packages=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name' | sort)
    else
      packages=$(tr ' ' '\n' <<< "$RUST_PACKAGES")
    fi
    RUST_PACKAGES=$( { grep -vE '^(sync_service|)$' <<< "$packages" || true; } \
      | awk -v shard="$TEST_SHARD" -v count="$TEST_SHARD_COUNT" '(NR - 1) % count == shard - 1' \
      | tr '\n' ' ' | sed 's/ $//')
    if [ -z "$RUST_PACKAGES" ]; then
      echo "No packages in test shard $TEST_SHARD of $TEST_SHARD_COUNT"
      exit 0
    fi
    echo "Test shard $TEST_SHARD of $TEST_SHARD_COUNT: $RUST_PACKAGES"
    export RUST_PACKAGES
"#};

/// Below the job's timeout, so the steps after it still report.
const RUN_TESTS_TIMEOUT_MINUTES: u32 = 150;

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

/// Fetches the blobs that the diff against the base revision reads, before the
/// dev shell. The web job's checkout has no blobs, and `vitest --changed` runs
/// `git diff --name-only -z <base>...HEAD`, whose rename detection reads the
/// blobs of added and deleted files. Inside the dev shell, git fetches them
/// with the runner's git-remote-https under Nix's LD_LIBRARY_PATH, which
/// aborts. The same diff here, outside it, fetches them in one batch.
fn fetch_diff_blobs() -> Step<Run> {
    Step::new("fetch the blobs of the change's diff")
        .run(indoc::indoc! {r#"
            set -euo pipefail
            if [ ! -f /tmp/base-revision ]; then
              exit 0
            fi
            git diff --name-only -z "$(cat /tmp/base-revision)...HEAD" > /dev/null
        "#})
        .if_condition(Expression::new("github.event_name == 'pull_request'"))
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
