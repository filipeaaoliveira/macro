use super::*;

/// Any other trigger (`pull_request_target`, `issue_comment`, `workflow_run`)
/// can run code from another repository with this repository's token.
#[test]
fn only_pull_requests_and_manual_runs_start_it() {
    let workflow = serde_json::to_value(fork_ci()).unwrap();
    let mut triggers: Vec<&str> = workflow["on"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    triggers.sort_unstable();

    assert_eq!(triggers, ["pull_request", "workflow_dispatch"]);
    assert_eq!(
        workflow["permissions"],
        serde_json::json!({ "contents": "read" })
    );
}

/// Every job runs on a hosted runner, and the Rust jobs only for a manual run
/// or a branch of this repository.
#[test]
fn every_job_runs_hosted_and_rust_jobs_only_trusted_code() {
    let workflow = serde_json::to_value(fork_ci()).unwrap();
    let jobs = workflow["jobs"].as_object().unwrap();

    for (id, job) in jobs {
        assert_eq!(job["runs-on"], HOSTED_RUNNER, "{id}");
    }

    for id in ["rust-check", "rust-test"] {
        let condition = jobs[id]["if"].as_str().unwrap();
        assert!(
            condition.ends_with(
                "&& (github.event_name == 'workflow_dispatch' || \
                 github.event.pull_request.head.repo.full_name == github.repository)"
            ),
            "{id}: {condition}"
        );
    }
}

/// Teardown Nix kills every process on the file system that holds /nix, for
/// a cache volume to unmount. On a hosted runner that is the root file
/// system, the runner's own processes included, so no job runs it.
#[test]
fn no_job_tears_down_nix() {
    let workflow = serde_json::to_value(fork_ci()).unwrap();
    for (id, job) in workflow["jobs"].as_object().unwrap() {
        let names: Vec<&str> = job["steps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|step| step["name"].as_str())
            .collect();
        assert!(!names.contains(&"Teardown Nix"), "{id}: {names:?}");
    }
}

/// The test shards come from `changes`, and a failed shard fails the others'
/// job result only at the end, so every shard reports.
#[test]
fn the_test_job_runs_one_shard_per_counted_share() {
    let job = serde_json::to_value(rust_test()).unwrap();

    assert_eq!(
        job["strategy"]["matrix"]["shard"],
        "${{ fromJSON(needs.changes.outputs.test_shards) }}"
    );
    assert_eq!(job["strategy"]["fail-fast"], false);
}

/// The required check runs on every run and waits for every other job, so a
/// failed, cancelled or untrusted run can't pass it by skipping it.
#[test]
fn the_gate_always_runs_after_every_job() {
    let workflow = serde_json::to_value(fork_ci()).unwrap();
    let jobs = workflow["jobs"].as_object().unwrap();
    let gate = &jobs["gate"];

    assert_eq!(gate["name"], "Fork CI gate");
    assert_eq!(gate["if"], "always()");
    assert_eq!(gate["runs-on"], HOSTED_RUNNER);

    let others: Vec<&str> = jobs
        .keys()
        .map(String::as_str)
        .filter(|id| *id != "gate")
        .collect();
    assert_eq!(gate["needs"], serde_json::json!(others));
}

/// `changes` builds the package filter from the pull request's code, so its
/// scripts run only for a trusted pull request. Counting the test shards reads
/// only their output.
#[test]
fn changes_runs_scripts_only_for_trusted_pull_requests() {
    let job = serde_json::to_value(changes()).unwrap();
    let scripts: Vec<&serde_json::Value> = job["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|step| step.get("run").is_some())
        .filter(|step| step["id"] != "test-shards")
        .collect();

    assert_eq!(scripts.len(), 2);

    for step in scripts {
        let condition = step["if"].as_str().unwrap();
        assert!(
            condition.starts_with(
                "github.event_name == 'pull_request' && (github.event_name == 'workflow_dispatch' || \
                 github.event.pull_request.head.repo.full_name == github.repository)"
            ),
            "{}: {condition}",
            step["name"]
        );
    }
}

/// A manual run checks every package, with the live-Postgres tests.
#[test]
fn a_manual_run_checks_every_package() {
    let job = serde_json::to_value(changes()).unwrap();

    assert!(
        job["outputs"]["rust_packages"]
            .as_str()
            .unwrap()
            .starts_with("${{ github.event_name == 'workflow_dispatch' && 'all' ||")
    );
    assert!(
        job["outputs"]["skip_tests"]
            .as_str()
            .unwrap()
            .starts_with("${{ github.event_name == 'pull_request' &&")
    );
}

/// The web job's checkout has no blobs, and git fetches missing ones with a
/// helper that aborts inside the dev shell, so every step that makes git read
/// the change's blobs runs before Nix is set up.
#[test]
fn the_web_job_fetches_the_diff_s_blobs_before_the_dev_shell() {
    let job = serde_json::to_value(web()).unwrap();
    let names: Vec<&str> = job["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["name"].as_str().unwrap_or_default())
        .collect();
    let position = |name: &str| {
        names
            .iter()
            .position(|step| *step == name)
            .unwrap_or_else(|| panic!("no step {name}: {names:?}"))
    };

    assert!(position("fetch the blobs of the change's diff") < position("Setup Nix"));
    assert!(position("compute changed files") < position("fetch the blobs of the change's diff"));
}

/// The web job holds about 7 GB, which the self-hosted runner's node cannot
/// hold beside a test build, so it runs on a hosted runner, beside the Rust
/// jobs instead of after them.
#[test]
fn the_web_job_runs_on_a_hosted_runner() {
    let job = serde_json::to_value(web()).unwrap();

    assert_eq!(job["runs-on"], HOSTED_RUNNER);
}

/// `rust-cache` keys its entries on every `RUST*` variable it sees, and looks
/// for target directories inside the workspace. A job-wide `RUST_PACKAGES`
/// would key each set of packages apart, and an absolute target directory
/// would leave the compiled dependencies out of the cache.
#[test]
fn rust_cache_sees_one_key_per_job_and_its_target_directory() {
    let workflow = serde_json::to_value(fork_ci()).unwrap();
    for id in ["rust-check", "rust-test"] {
        let job = &workflow["jobs"][id];
        assert!(job["env"].get("RUST_PACKAGES").is_none(), "{id}");

        let cache = job["steps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["name"] == "cache compiled dependencies")
            .unwrap();
        let workspaces = cache["with"]["workspaces"].as_str().unwrap();
        let target = workspaces.split(" -> ").nth(1).unwrap();
        assert!(!target.starts_with('/'), "{id}: {workspaces}");
    }
}
