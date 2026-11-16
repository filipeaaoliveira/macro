use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use factory_records::domain::models::{HandBack, Outcome, RunId};
use macro_authorization::{
    INTERNAL_API_KEY_HEADER, INTERNAL_MACRO_USER_ID_HEADER, InternalAuthConfig,
    MacroAuthorizationServiceImpl, MacroAuthorizationState, NoBotAuthorizer,
    NoUserApiKeyAuthorizer, NoopMacroAuthJwtValidator,
};
use macro_user_id::user_id::MacroUserIdStr;
use tower::ServiceExt as _;
use upstream_error::{UpstreamError, UpstreamFailure};
use uuid::Uuid;

use super::{FactoryRouterState, factory_router};
use crate::domain::models::{FactoryError, RunDetail};
use crate::domain::ports::FactoryService;

const KEY: &str = "test-internal-key";

/// A Run API that answers every call with what `answer` makes, and keeps
/// whom each call acted for.
struct FakeService {
    answer: fn() -> FactoryError,
    actors: Mutex<Vec<String>>,
}

impl FakeService {
    fn refusing(&self, actor: &MacroUserIdStr<'static>) -> FactoryError {
        self.actors.lock().expect("actors").push(actor.to_string());
        (self.answer)()
    }
}

impl FactoryService for FakeService {
    async fn create_run(
        &self,
        actor: MacroUserIdStr<'static>,
        _definition: serde_json::Value,
        _request: String,
        _task_id: Option<Uuid>,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn get_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        _id: RunId,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn link_task(
        &self,
        actor: &MacroUserIdStr<'static>,
        _id: RunId,
        _task_id: Uuid,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn answer_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        _id: RunId,
        _seq: u16,
        _answer: Outcome,
        _note: Option<String>,
    ) -> Result<(), FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn hand_back(
        &self,
        _session_id: Uuid,
        user_id: MacroUserIdStr<'static>,
        _hand_back: HandBack,
    ) -> Result<(), FactoryError> {
        Err(self.refusing(&user_id))
    }

    async fn cancel_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        _id: RunId,
    ) -> Result<(), FactoryError> {
        Err(self.refusing(actor))
    }
}

fn router(answer: fn() -> FactoryError) -> (Router, Arc<FakeService>) {
    let service = Arc::new(FakeService {
        answer,
        actors: Mutex::new(Vec::new()),
    });
    let authorization = MacroAuthorizationServiceImpl::new(
        NoopMacroAuthJwtValidator,
        InternalAuthConfig {
            api_key: KEY.to_string(),
            default_user_id: None,
        },
        NoBotAuthorizer,
        NoUserApiKeyAuthorizer,
    );
    let state = FactoryRouterState::new(
        Arc::clone(&service),
        MacroAuthorizationState::new(Arc::new(authorization)),
    );
    (factory_router(state), service)
}

/// An internal call, for `acting_user` when one is named.
fn call(
    method: &str,
    path: &str,
    acting_user: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(INTERNAL_API_KEY_HEADER, KEY);
    if let Some(user) = acting_user {
        request = request.header(INTERNAL_MACRO_USER_ID_HEADER, user);
    }
    request.body(Body::from(body.to_string())).expect("request")
}

async fn body(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn an_internal_call_acts_for_its_acting_user_and_never_without_one() {
    let (router, service) = router(|| FactoryError::NotFound);
    let run = format!("/runs/{}", RunId::generate());

    let acted = router
        .clone()
        .oneshot(call(
            "GET",
            &run,
            Some("macro|julia@example.com"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    let anonymous = router
        .oneshot(call("GET", &run, None, serde_json::Value::Null))
        .await
        .expect("response");

    assert_eq!(acted.status(), StatusCode::NOT_FOUND);
    assert_eq!(anonymous.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        *service.actors.lock().expect("actors"),
        ["macro|julia@example.com"]
    );
}

/// The acting user's id is lowercased once, where the call comes in, so a
/// Run created for `macro|Julia@Example.com` is owned by the account the
/// lowercased `"User".id` names, and owner checks compare like with like.
#[tokio::test]
async fn a_run_is_created_for_the_lowercased_acting_user() {
    let (router, service) = router(|| FactoryError::EmptyRequest);

    let response = router
        .oneshot(call(
            "POST",
            "/runs",
            Some("macro|Julia@Example.com"),
            serde_json::json!({ "flow": {}, "request": "Fix the login bug" }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        *service.actors.lock().expect("actors"),
        ["macro|julia@example.com"]
    );
}

/// The caller of a Run that Temporal can't reach is told to try again
/// later, in the shape every Macro client reads an upstream failure in.
#[tokio::test]
async fn a_workflow_outage_tells_the_caller_to_try_again() {
    let (router, _) = router(|| {
        FactoryError::Upstream(UpstreamError {
            service: "The workflow service",
            failure: UpstreamFailure::Unavailable,
        })
    });

    let response = router
        .oneshot(call(
            "POST",
            "/runs",
            Some("macro|julia@example.com"),
            serde_json::json!({ "flow": {}, "request": "Fix the login bug" }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body(response).await["code"], "upstream_unavailable");
}

/// A wrong answer is the caller's to fix; a Gate that stopped waiting is a
/// conflict with the Run's state, which reading the Run explains.
#[tokio::test]
async fn a_wrong_answer_is_the_callers_mistake_and_a_late_one_a_conflict() {
    let answer = |router: Router| async move {
        router
            .oneshot(call(
                "POST",
                &format!("/runs/{}/steps/1/answer", RunId::generate()),
                Some("macro|julia@example.com"),
                serde_json::json!({ "answer": "approve" }),
            ))
            .await
            .expect("response")
            .status()
    };

    let wrong = answer(
        router(|| FactoryError::AnswerNotOffered {
            offered: vec!["revise".to_string()],
        })
        .0,
    )
    .await;
    let late = answer(router(|| FactoryError::AnswerRefused("not waiting".to_string())).0).await;

    assert!(wrong.is_client_error() && wrong != StatusCode::CONFLICT);
    assert_eq!(late, StatusCode::CONFLICT);
}
