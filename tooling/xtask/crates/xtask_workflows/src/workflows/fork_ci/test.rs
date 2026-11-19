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

/// A job on the self-hosted runner runs only for a manual run or a branch of
/// this repository.
#[test]
fn self_hosted_jobs_run_only_trusted_code() {
    let workflow = serde_json::to_value(fork_ci()).unwrap();
    let jobs = workflow["jobs"].as_object().unwrap();
    let self_hosted: Vec<&String> = jobs
        .iter()
        .filter(|(_, job)| job["runs-on"] == RUNNER)
        .map(|(id, _)| id)
        .collect();

    assert_eq!(self_hosted, ["rust-check", "rust-test", "web"]);

    for id in self_hosted {
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
/// scripts run only for a trusted pull request.
#[test]
fn changes_runs_scripts_only_for_trusted_pull_requests() {
    let job = serde_json::to_value(changes()).unwrap();
    let scripts: Vec<&serde_json::Value> = job["steps"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|step| step.get("run").is_some())
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
