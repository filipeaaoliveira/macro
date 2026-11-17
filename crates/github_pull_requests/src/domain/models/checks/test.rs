use super::GithubWorkflowRunStatus;

/// Only `completed` is finished: a status GitHub adds later, or none at
/// all, is taken as a run that still waits.
#[test]
fn a_workflow_run_status_other_than_completed_waits() {
    assert_eq!(
        GithubWorkflowRunStatus::from_github("completed"),
        GithubWorkflowRunStatus::Completed
    );
    for status in ["queued", "pending", "action_required", ""] {
        assert!(
            GithubWorkflowRunStatus::from_github(status).is_waiting(),
            "{status:?}"
        );
    }
}
