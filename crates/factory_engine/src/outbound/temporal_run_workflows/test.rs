use temporalio_client::errors::{WorkflowInteractionError, WorkflowUpdateError};
use temporalio_client::tonic::Status;

use super::{cancelled, delivery, from_status};
use crate::domain::ports::{AnswerDelivery, RunWorkflowsError};

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

/// A Run whose workflow completed before its end was recorded takes no
/// answer and no cancel: both are the caller's conflict, not our failure.
#[test]
fn a_completed_workflow_refuses_an_answer_and_a_cancel() {
    let answered = delivery(Err(WorkflowUpdateError::NotFound(Status::not_found(
        "workflow execution already completed",
    ))));
    let cancel = cancelled(Err(WorkflowInteractionError::NotFound(Status::not_found(
        "workflow execution already completed",
    ))));

    assert!(matches!(answered, Ok(AnswerDelivery::Refused(_))));
    assert!(matches!(cancel, Ok(false)));
}
