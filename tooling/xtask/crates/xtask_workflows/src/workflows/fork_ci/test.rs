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

    assert_eq!(self_hosted, ["rust", "web"]);

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
