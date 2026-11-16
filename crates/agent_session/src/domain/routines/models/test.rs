use super::RoutineSessionError;

/// Only an outage passes on a retry: a refusal, a mismatch or a conflict
/// answers the same way again, and an uncertain prompt must not be resent.
#[test]
fn only_an_outage_is_transient() {
    for (error, transient) in [
        (RoutineSessionError::RuntimeUnavailable, true),
        (RoutineSessionError::OperationFailed, true),
        (RoutineSessionError::InvalidCommand, false),
        (RoutineSessionError::PersonaUnavailable, false),
        (RoutineSessionError::Forbidden, false),
        (RoutineSessionError::AppNotConnected, false),
        (RoutineSessionError::AppsUnsupported, false),
        (RoutineSessionError::TeamAgentComputer, false),
        (RoutineSessionError::SessionMismatch, false),
        (RoutineSessionError::ModelMismatch, false),
        (RoutineSessionError::Conflict, false),
        (RoutineSessionError::PromptDeliveryUnknown, false),
    ] {
        assert_eq!(error.is_transient(), transient, "{error:?}");
    }
}

/// Billing that could not check the request may pass later; a denial won't.
#[cfg(feature = "admission")]
#[test]
fn only_an_unavailable_admission_check_is_transient() {
    use crate::domain::error::AiAdmissionError;

    assert!(RoutineSessionError::Admission(AiAdmissionError::Unavailable).is_transient());
    let denied =
        AiAdmissionError::Denied(ai_billing::domain::models::DenyReason::AllowanceExhausted);
    assert!(!RoutineSessionError::Admission(denied).is_transient());
}
