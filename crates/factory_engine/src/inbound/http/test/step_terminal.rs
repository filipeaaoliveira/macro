//! The Step Terminal's routes: only the Run's owner in their own signed-in
//! session steers a Step. Code mode, a factory Step's own session among
//! them, an internal caller and a user API key are refused before the
//! service hears of it; and a Step that takes no steering says so.

use agent_correlation::context::{Attribution, scope_attribution};
use agent_correlation::scope::SessionScope;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use factory_records::domain::models::{RunId, SteerRefusal};
use macro_authorization::CODE_EXECUTION_HEADER;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{ALICE, Caller, call, router};
use crate::domain::models::FactoryError;

/// Each Step Terminal route of Step 1 of `run`, with its body.
fn routes(run: RunId) -> [(String, serde_json::Value); 4] {
    [
        (
            format!("/runs/{run}/steps/1/note"),
            serde_json::json!({ "note": "Keep the API" }),
        ),
        (format!("/runs/{run}/steps/1/pause"), serde_json::json!({})),
        (format!("/runs/{run}/steps/1/resume"), serde_json::json!({})),
        (format!("/runs/{run}/steps/1/stop"), serde_json::json!({})),
    ]
}

/// The call `request` made from the code of a factory Step's agent session,
/// as the correlation layer accepts it: held to the scope a Step's session
/// runs in.
async fn from_a_step_session(router: &Router, request: Request<Body>) -> StatusCode {
    let attribution = Attribution {
        session_id: Some(Uuid::now_v7()),
        scope: Some(SessionScope::NoSend),
        ..Attribution::default()
    };
    scope_attribution(attribution, router.clone().oneshot(request))
        .await
        .expect("response")
        .status()
}

#[tokio::test]
async fn only_the_owner_in_person_steers_a_step() {
    let (router, service) = router(|| FactoryError::NotFound);
    let run = RunId::generate();

    for (path, body) in routes(run) {
        let person = router
            .clone()
            .oneshot(call("POST", &path, Caller::User("alice"), body.clone()))
            .await
            .expect("response");
        assert_eq!(person.status(), StatusCode::NOT_FOUND, "{path}");

        let mut code = call("POST", &path, Caller::User("alice"), body.clone());
        code.headers_mut()
            .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
        assert_eq!(
            from_a_step_session(&router, code).await,
            StatusCode::FORBIDDEN,
            "a step's session steers nothing: {path}"
        );
        let internal = router
            .clone()
            .oneshot(call(
                "POST",
                &path,
                Caller::Internal(Some(ALICE)),
                body.clone(),
            ))
            .await
            .expect("response");
        assert_eq!(internal.status(), StatusCode::FORBIDDEN, "{path}");
        let key = router
            .clone()
            .oneshot(call(
                "POST",
                &path,
                Caller::ApiKey("alice-key"),
                body.clone(),
            ))
            .await
            .expect("response");
        assert_eq!(key.status(), StatusCode::FORBIDDEN, "{path}");
    }

    // Only the four calls in person reached the service.
    assert_eq!(service.actors.lock().expect("actors").len(), 4);
}

#[tokio::test]
async fn a_step_that_takes_no_steering_is_a_conflict() {
    let (router, _) = router(|| FactoryError::NotSteerable(SteerRefusal::Paused));
    let run = RunId::generate();
    let (path, body) = routes(run)[0].clone();

    let refused = router
        .oneshot(call("POST", &path, Caller::User("alice"), body))
        .await
        .expect("response");

    assert_eq!(refused.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_session_that_did_not_take_the_note_is_a_bad_gateway() {
    let (router, _) = router(|| FactoryError::SessionUnreachable);
    let run = RunId::generate();
    let (path, body) = routes(run)[0].clone();

    let refused = router
        .oneshot(call("POST", &path, Caller::User("alice"), body))
        .await
        .expect("response");

    assert_eq!(refused.status(), StatusCode::BAD_GATEWAY);
}
