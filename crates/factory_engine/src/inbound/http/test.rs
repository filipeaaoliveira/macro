use axum::http::StatusCode;
use axum::response::IntoResponse;
use upstream_error::{UpstreamError, UpstreamFailure};

use super::required;
use crate::domain::models::FactoryError;

#[test]
fn an_internal_call_with_no_acting_user_is_refused() {
    let refused = required(None);

    assert!(matches!(refused, Err(FactoryError::NoActingUser)));
    assert_eq!(
        FactoryError::NoActingUser.into_response().status(),
        StatusCode::BAD_REQUEST
    );
}

#[test]
fn each_error_answers_with_the_status_its_cause_calls_for() {
    let cases = [
        (FactoryError::NotFound, StatusCode::NOT_FOUND),
        (FactoryError::UnknownUser, StatusCode::BAD_REQUEST),
        (
            FactoryError::AnswerNotOffered {
                offered: vec!["approve".to_string()],
            },
            StatusCode::BAD_REQUEST,
        ),
        (
            FactoryError::AnswerRefused("not waiting".to_string()),
            StatusCode::CONFLICT,
        ),
        (FactoryError::RunEnded, StatusCode::CONFLICT),
        (FactoryError::NotTheOwner, StatusCode::FORBIDDEN),
        (
            FactoryError::Upstream(UpstreamError {
                service: "Temporal",
                failure: UpstreamFailure::Unavailable,
            }),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            FactoryError::Upstream(UpstreamError {
                service: "Temporal",
                failure: UpstreamFailure::InvalidResponse,
            }),
            StatusCode::BAD_GATEWAY,
        ),
        (
            FactoryError::Internal(rootcause::report!("broken")),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];

    for (error, status) in cases {
        let shown = format!("{error}");
        assert_eq!(error.into_response().status(), status, "{shown}");
    }
}
