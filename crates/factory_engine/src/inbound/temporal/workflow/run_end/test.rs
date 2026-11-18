use factory_records::domain::models::{Outcome, RunOutcome};

use super::RunEnding;
use crate::inbound::temporal::workflow::review_round::Round;

#[test]
fn a_run_failed_on_work_that_failed_for_infrastructure_ends_so() {
    let mut ending = RunEnding::default();
    ending.routed(&Round::FailedForInfrastructure);

    assert!(ending.infrastructure(RunOutcome::Failed, false));
    assert!(!ending.infrastructure(RunOutcome::Blocked, false));
    assert!(!ending.infrastructure(RunOutcome::Manual, false));
}

/// Work that failed for infrastructure, routed on to a later Step whose
/// own work then fails, ends the Run as that later failure does.
#[test]
fn a_later_failure_of_the_work_ends_the_run_failed() {
    let mut ending = RunEnding::default();
    ending.routed(&Round::FailedForInfrastructure);
    ending.routed(&Round::Routed(Outcome::failed()));

    assert!(!ending.infrastructure(RunOutcome::Failed, false));
}

/// A Flow that could not go on fails the Run for itself, whatever the
/// last round was.
#[test]
fn a_flow_that_stopped_ends_the_run_failed() {
    let mut ending = RunEnding::default();
    ending.routed(&Round::FailedForInfrastructure);

    assert!(!ending.infrastructure(RunOutcome::Failed, true));
}
