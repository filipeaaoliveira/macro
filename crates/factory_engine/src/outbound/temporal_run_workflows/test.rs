use temporalio_client::tonic::Status;

use super::from_status;
use crate::domain::ports::RunWorkflowsError;

#[test]
fn temporal_out_of_reach_is_its_outage_and_anything_else_is_ours() {
    let outages = [
        Status::unavailable("connection refused"),
        Status::deadline_exceeded("timed out"),
        Status::resource_exhausted("throttled"),
    ];
    for status in outages {
        assert!(
            matches!(
                from_status(status.clone()),
                RunWorkflowsError::Unavailable(_)
            ),
            "{status:?}"
        );
    }

    let ours = from_status(Status::invalid_argument("bad input"));
    assert!(matches!(ours, RunWorkflowsError::Other(_)));
}
