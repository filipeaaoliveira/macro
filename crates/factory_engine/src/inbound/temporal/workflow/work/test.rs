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
use crate::inbound::temporal::activities::{AGENT_OVERDUE_FAILURE, INFRASTRUCTURE_FAILURE};

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

/// The attempts ran out of time, the last of them having failed with
/// `last`.
fn timed_out(last: Failure) -> Failure {
    Failure {
        message: "activity timeout".to_string(),
        failure_info: Some(FailureInfo::TimeoutFailureInfo(TimeoutFailureInfo {
            timeout_type: TimeoutType::ScheduleToClose.into(),
            ..TimeoutFailureInfo::default()
        })),
        cause: Some(Box::new(last)),
        ..Failure::default()
    }
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
    let ran_out_of_time = activity(timed_out(application(None, false)));
    let agent_too_slow = activity(timed_out(application(Some(AGENT_OVERDUE_FAILURE), false)));
    let bad_outcome = activity(application(None, true));

    assert_eq!(work_failure(&out_of_quota), WorkFailure::Infrastructure);
    assert_eq!(work_failure(&ran_out_of_time), WorkFailure::Infrastructure);
    assert_eq!(work_failure(&agent_too_slow), WorkFailure::Work);
    assert_eq!(work_failure(&bad_outcome), WorkFailure::Work);
}
