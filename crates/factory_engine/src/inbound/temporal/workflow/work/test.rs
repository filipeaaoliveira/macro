//! Why a Step's work failed for good, read from activity failures as the
//! workflow's history would hand them back.

use temporalio_common::data_converters::{
    ActivityExecutionDecodeHint, DefaultFailureConverter, FailureConverter, FailureDecodeHint,
    PayloadConverter, SerializationContextData,
};
use temporalio_common::error::ActivityExecutionError;
use temporalio_common::protos::temporal::api::enums::v1::TimeoutType;
use temporalio_common::protos::temporal::api::failure::v1::{
    ActivityFailureInfo, ApplicationFailureInfo, Failure, TimeoutFailureInfo, failure::FailureInfo,
};

use super::{WorkFailure, work_failure};
use crate::inbound::temporal::activities::{
    AGENT_OVERDUE_FAILURE, INFRASTRUCTURE_FAILURE, REFUSED_FAILURE,
};

/// An application failure of `kind`, if any, as an activity raised it.
fn application(kind: Option<&str>, non_retryable: bool) -> Failure {
    Failure {
        message: "the step's work failed".to_string(),
        failure_info: Some(FailureInfo::ApplicationFailureInfo(
            ApplicationFailureInfo {
                r#type: kind.unwrap_or_default().to_string(),
                non_retryable,
                ..ApplicationFailureInfo::default()
            },
        )),
        ..Failure::default()
    }
}

/// A timeout of `kind`, caused by `last`, if any: the attempts ran out of
/// time, the last of them having failed with it.
fn timeout(kind: TimeoutType, last: Option<Failure>) -> Failure {
    Failure {
        message: "activity timeout".to_string(),
        failure_info: Some(FailureInfo::TimeoutFailureInfo(TimeoutFailureInfo {
            timeout_type: kind.into(),
            ..TimeoutFailureInfo::default()
        })),
        cause: last.map(Box::new),
        ..Failure::default()
    }
}

/// The attempts ran out of time, the last of them having failed with
/// `last`.
fn timed_out(last: Failure) -> Failure {
    timeout(TimeoutType::ScheduleToClose, Some(last))
}

/// The activity failure the workflow reads, caused by `cause`.
fn activity(cause: Failure) -> ActivityExecutionError {
    let failure = Failure {
        message: "activity failed".to_string(),
        failure_info: Some(FailureInfo::ActivityFailureInfo(
            ActivityFailureInfo::default(),
        )),
        cause: Some(Box::new(cause)),
        ..Failure::default()
    };
    let error = DefaultFailureConverter::default()
        .to_error(
            failure,
            &PayloadConverter::default(),
            &SerializationContextData::None,
        )
        .expect("the failure decodes");
    ActivityExecutionDecodeHint::new(false).adapt(error)
}

#[test]
fn only_what_is_surely_the_factory_s_counts_as_infrastructure() {
    let out_of_quota = activity(application(Some(INFRASTRUCTURE_FAILURE), true));
    let no_last_failure = activity(timeout(TimeoutType::ScheduleToClose, None));
    let worker_lost = activity(timed_out(timeout(TimeoutType::Heartbeat, None)));
    let attempt_too_long = activity(timeout(TimeoutType::StartToClose, None));
    let last_attempt_too_long = activity(timed_out(timeout(TimeoutType::StartToClose, None)));
    let never_taken = activity(timeout(TimeoutType::ScheduleToStart, None));
    let quota_at_the_end = activity(timed_out(application(Some(INFRASTRUCTURE_FAILURE), false)));
    let retried_work = activity(timed_out(application(None, false)));
    // As Temporal hands back attempts whose time ran out after a retried
    // failure: that failure, with no timeout around it.
    let out_of_retries = activity(application(None, false));
    let agent_too_slow = activity(timed_out(application(Some(AGENT_OVERDUE_FAILURE), false)));
    let bad_outcome = activity(application(None, true));

    assert_eq!(work_failure(&out_of_quota), WorkFailure::Infrastructure);
    assert_eq!(work_failure(&no_last_failure), WorkFailure::Infrastructure);
    assert_eq!(work_failure(&worker_lost), WorkFailure::Infrastructure);
    assert_eq!(work_failure(&never_taken), WorkFailure::Infrastructure);
    // An attempt that ran past its own time may be an agent slower than
    // its deadline: the work's.
    assert_eq!(work_failure(&attempt_too_long), WorkFailure::Work);
    assert_eq!(work_failure(&last_attempt_too_long), WorkFailure::Work);
    assert_eq!(work_failure(&quota_at_the_end), WorkFailure::Infrastructure);
    // When unsure, the failure is the work's: a retried failure of no
    // known kind, as a round of checks that could not go on.
    assert_eq!(work_failure(&retried_work), WorkFailure::Work);
    assert_eq!(work_failure(&out_of_retries), WorkFailure::Work);
    assert_eq!(work_failure(&agent_too_slow), WorkFailure::Work);
    assert_eq!(work_failure(&bad_outcome), WorkFailure::Work);
}

/// A session its agent's machine refused is infrastructure's, and the
/// refusal, the failure's message, is kept for the Step's output, however
/// the attempts ended.
#[test]
fn a_refused_session_is_infrastructure_s_and_keeps_its_refusal() {
    let refused = activity(application(Some(REFUSED_FAILURE), true));
    let refused_at_the_end = activity(timed_out(application(Some(REFUSED_FAILURE), true)));

    for failure in [refused, refused_at_the_end] {
        let read = work_failure(&failure);
        assert_eq!(read, WorkFailure::Refused("the step's work failed".to_string()));
        assert!(read.infrastructure());
        assert_eq!(read.output(), "the step's work failed");
    }
    assert!(!WorkFailure::Work.infrastructure());
}
