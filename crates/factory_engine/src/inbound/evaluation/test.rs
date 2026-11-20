use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use agent_correlation::context::{Attribution, scope_attribution};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use factory_records::domain::evaluation::AgentChoices;
use macro_authorization::{
    AuthorizedUserApiKey, CODE_EXECUTION_HEADER, CredentialOrigin, INTERNAL_API_KEY_HEADER,
    INTERNAL_MACRO_USER_ID_HEADER, InternalAuthConfig, JwtValidator, MacroAuthorizationError,
    MacroAuthorizationServiceImpl, MacroAuthorizationState, NoBotAuthorizer, UserApiKeyAuthorizer,
    ValidatedCredential, ValidatedIdentity,
};
use macro_user_id::user_id::MacroUserIdStr;
use rootcause::Report;
use tower::ServiceExt as _;
use uuid::Uuid;

use super::{EvaluationRouterState, evaluation_router};
use crate::domain::evaluation::EvaluationDefinitionError;
use crate::domain::evaluation::insights::{
    DismissalUndoneResponse, EvaluationInsights, EvaluationService, SuiteError, SuiteStarted,
    UndoDismissalError,
};
use crate::domain::insights::{InsightsError, InsightsQuery};
use crate::domain::models::{ActingAgent, FactoryError};

const KEY: &str = "test-internal-key";
const ALICE: &str = "macro|alice@example.com";
const EVE: &str = "macro|eve@example.com";

/// Accepts the bearer token `alice`, Alice's own session.
#[derive(Clone, Copy)]
struct Tokens;

impl JwtValidator for Tokens {
    fn validate_credential(
        &self,
        jwt: &str,
    ) -> Result<ValidatedCredential, Report<MacroAuthorizationError>> {
        if jwt != "alice" {
            return Err(Report::new(MacroAuthorizationError::InvalidCredentials));
        }
        Ok(ValidatedCredential {
            origin: CredentialOrigin::Session,
            identity: ValidatedIdentity {
                agent_id: None,
                agent_via: None,
                routine_id: None,
                user_id: ALICE.to_string(),
                fusion_user_id: "fusion-alice".to_string(),
                organization_id: None,
                permissions: None,
            },
        })
    }
}

/// Accepts no user API key.
#[derive(Clone, Copy)]
struct NoApiKeys;

impl UserApiKeyAuthorizer for NoApiKeys {
    async fn authorize_user_api_key(
        &self,
        _api_key: &str,
    ) -> Result<AuthorizedUserApiKey, Report<MacroAuthorizationError>> {
        Err(Report::new(MacroAuthorizationError::InvalidCredentials))
    }
}

/// Who asked, and for what: the insights' callers and the suites' starters
/// with their Benchmark.
#[derive(Default)]
struct FakeEvaluation {
    read_by: Mutex<Vec<String>>,
    started_by: Mutex<Vec<(String, String)>>,
    acting: Mutex<Vec<Option<ActingAgent>>>,
    undone_by: Mutex<Vec<(String, Uuid)>>,
}

/// The improvement `undo_dismissal` finds; [`ELSEWHERE`] is no Run of the
/// caller's team, [`NOT_REJECTED`] dismissed nothing, and [`NOT_YOURS`] is
/// one the caller may not undo.
const UNDOABLE: Uuid = Uuid::from_u128(10);
const ELSEWHERE: Uuid = Uuid::from_u128(11);
const NOT_REJECTED: Uuid = Uuid::from_u128(12);
const NOT_YOURS: Uuid = Uuid::from_u128(13);

/// `core` starts; `missing` is no Benchmark; `bad` names an unknown Agent;
/// `busy` still runs; `unreachable` is a repository the caller cannot reach.
impl EvaluationService for Arc<FakeEvaluation> {
    async fn evaluation(
        &self,
        actor: &MacroUserIdStr<'static>,
        query: InsightsQuery,
    ) -> Result<EvaluationInsights, InsightsError> {
        self.read_by
            .lock()
            .expect("read")
            .push(actor.as_ref().to_string());
        Ok(EvaluationInsights {
            from: query.from(),
            to: query.to(),
            time_zone: query.time_zone().to_string(),
            flow: None,
            scorers: Vec::new(),
            experiments: Vec::new(),
            benchmarks: Vec::new(),
            suites: Vec::new(),
            self_improvement: None,
        })
    }

    async fn start_suite(
        &self,
        actor: MacroUserIdStr<'static>,
        benchmark: &str,
        _agents: AgentChoices,
        acting: Option<ActingAgent>,
    ) -> Result<SuiteStarted, SuiteError> {
        self.acting.lock().expect("acting").push(acting);
        self.started_by
            .lock()
            .expect("started")
            .push((actor.as_ref().to_string(), benchmark.to_string()));
        match benchmark {
            "missing" => Err(SuiteError::UnknownBenchmark(benchmark.to_string())),
            "bad" => Err(SuiteError::Configuration(EvaluationDefinitionError::Arms(
                "bad".to_string(),
            ))),
            "busy" => Err(SuiteError::Running(benchmark.to_string())),
            "unreachable" => Err(SuiteError::Run(FactoryError::UnreachableRepository)),
            _ => Ok(SuiteStarted {
                id: Uuid::from_u128(1),
                runs: BTreeMap::new(),
                refused: None,
            }),
        }
    }

    async fn undo_dismissal(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: Uuid,
    ) -> Result<DismissalUndoneResponse, UndoDismissalError> {
        self.undone_by
            .lock()
            .expect("undone")
            .push((actor.as_ref().to_string(), id));
        match id {
            ELSEWHERE => Err(UndoDismissalError::NotFound),
            NOT_REJECTED => Err(UndoDismissalError::NothingDismissed),
            NOT_YOURS => Err(UndoDismissalError::NotAllowed),
            _ => Ok(DismissalUndoneResponse {
                restored: 2,
                undone_at: chrono::Utc::now(),
            }),
        }
    }
}

fn undo(id: Uuid) -> axum::http::request::Builder {
    Request::builder()
        .method("POST")
        .uri(format!("/self-improvements/{id}/undo-dismissal"))
}

fn app() -> (Router, Arc<FakeEvaluation>) {
    let service = Arc::new(FakeEvaluation::default());
    let authorization = MacroAuthorizationState::new(Arc::new(MacroAuthorizationServiceImpl::new(
        Tokens,
        InternalAuthConfig {
            api_key: KEY.to_string(),
            default_user_id: None,
        },
        NoBotAuthorizer,
        NoApiKeys,
    )));
    let router = evaluation_router(EvaluationRouterState::new(
        Arc::new(service.clone()),
        authorization,
    ));
    (router, service)
}

const NOVEMBER: &str = "/insights/evaluation?from=2026-11-01&to=2026-11-30&timeZone=UTC";

fn start(benchmark: &str) -> axum::http::request::Builder {
    Request::builder()
        .method("POST")
        .uri(format!("/benchmarks/{benchmark}/suites"))
        .header("content-type", "application/json")
}

async fn status(app: &Router, request: Request<Body>) -> StatusCode {
    app.clone()
        .oneshot(request)
        .await
        .expect("response")
        .status()
}

#[tokio::test]
async fn a_call_with_no_user_reads_and_starts_nothing() {
    let (app, service) = app();

    let read = Request::builder()
        .uri(NOVEMBER)
        .body(Body::empty())
        .expect("request");
    let started = start("core")
        .header(INTERNAL_API_KEY_HEADER, KEY)
        .body(Body::from("{}"))
        .expect("request");

    assert_eq!(status(&app, read).await, StatusCode::UNAUTHORIZED);
    assert_eq!(status(&app, started).await, StatusCode::UNAUTHORIZED);
    assert!(service.read_by.lock().expect("read").is_empty());
    assert!(service.started_by.lock().expect("started").is_empty());
}

#[tokio::test]
async fn a_person_reads_and_starts_as_themselves_and_an_internal_call_as_its_user() {
    let (app, service) = app();

    let alice = Request::builder()
        .uri(NOVEMBER)
        .header("authorization", "Bearer alice")
        .body(Body::empty())
        .expect("request");
    let for_eve = start("core")
        .header(INTERNAL_API_KEY_HEADER, KEY)
        .header(INTERNAL_MACRO_USER_ID_HEADER, EVE)
        .body(Body::from(r#"{"agents":{"writer":"primary"}}"#))
        .expect("request");

    assert_eq!(status(&app, alice).await, StatusCode::OK);
    let response = app.clone().oneshot(for_eve).await.expect("response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let body: serde_json::Value = serde_json::from_slice(
        &to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body"),
    )
    .expect("json");
    assert_eq!(body["refused"], serde_json::Value::Null);
    assert_eq!(*service.read_by.lock().expect("read"), [ALICE]);
    assert_eq!(
        *service.started_by.lock().expect("started"),
        [(EVE.to_string(), "core".to_string())]
    );
}

#[tokio::test]
async fn a_refused_suite_says_why_with_its_status() {
    let (app, _service) = app();
    let cases = [
        ("missing", StatusCode::NOT_FOUND),
        ("bad", StatusCode::BAD_REQUEST),
        ("busy", StatusCode::CONFLICT),
        ("unreachable", StatusCode::BAD_REQUEST),
    ];

    for (benchmark, expected) in cases {
        let request = start(benchmark)
            .header("authorization", "Bearer alice")
            .body(Body::from("{}"))
            .expect("request");
        assert_eq!(status(&app, request).await, expected, "{benchmark}");
    }
    let unknown_field = start("core")
        .header("authorization", "Bearer alice")
        .body(Body::from(r#"{"agent":{}}"#))
        .expect("request");
    assert_eq!(
        status(&app, unknown_field).await,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

/// Each trial of a suite is a Run the caller starts, so the suite's start
/// reads who acts for them as `POST /runs` does: code in an agent session
/// reaches the service as that session, for Standing Roles to check; the
/// person in their own session and an internal caller as no agent.
#[tokio::test]
async fn an_agent_session_starts_a_suite_as_that_session() {
    let (app, service) = app();
    let session_id = Uuid::now_v7();
    let body = || Body::from(r#"{"agents":{"writer":"primary"}}"#);

    let from_code = start("core")
        .header("authorization", "Bearer alice")
        .header(CODE_EXECUTION_HEADER, "true")
        .body(body())
        .expect("request");
    let attribution = Attribution {
        session_id: Some(session_id),
        ..Attribution::default()
    };
    let in_session = scope_attribution(attribution, app.clone().oneshot(from_code))
        .await
        .expect("response");
    let in_person = start("core")
        .header("authorization", "Bearer alice")
        .body(body())
        .expect("request");
    let internal = start("core")
        .header(INTERNAL_API_KEY_HEADER, KEY)
        .header(INTERNAL_MACRO_USER_ID_HEADER, ALICE)
        .body(body())
        .expect("request");

    assert_eq!(in_session.status(), StatusCode::CREATED);
    assert_eq!(status(&app, in_person).await, StatusCode::CREATED);
    assert_eq!(status(&app, internal).await, StatusCode::CREATED);
    assert_eq!(
        *service.acting.lock().expect("acting"),
        [Some(ActingAgent::Session(session_id)), None, None]
    );
}

/// Only the person in their own signed-in session undoes a dismissal, as
/// themselves: an internal caller acting for them and an agent's code are
/// refused before the service is asked.
#[tokio::test]
async fn only_a_person_in_their_session_undoes_a_dismissal() {
    let (app, service) = app();

    let internal = undo(UNDOABLE)
        .header(INTERNAL_API_KEY_HEADER, KEY)
        .header(INTERNAL_MACRO_USER_ID_HEADER, ALICE)
        .body(Body::empty())
        .expect("request");
    let from_code = undo(UNDOABLE)
        .header("authorization", "Bearer alice")
        .header(CODE_EXECUTION_HEADER, "true")
        .body(Body::empty())
        .expect("request");
    assert_eq!(status(&app, internal).await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, from_code).await, StatusCode::FORBIDDEN);
    assert!(service.undone_by.lock().expect("undone").is_empty());

    let in_person = undo(UNDOABLE)
        .header("authorization", "Bearer alice")
        .body(Body::empty())
        .expect("request");
    let response = app.clone().oneshot(in_person).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert_eq!(body["restored"], 2);
    assert!(body["undoneAt"].is_string());
    assert_eq!(
        *service.undone_by.lock().expect("undone"),
        [(ALICE.to_string(), UNDOABLE)]
    );
}

/// A refused undo answers with its status: another team's Run is not
/// found, one that dismissed nothing conflicts, and one the caller may not
/// undo is forbidden.
#[tokio::test]
async fn a_refused_undo_says_why_with_its_status() {
    let (app, _) = app();
    for (id, expected) in [
        (ELSEWHERE, StatusCode::NOT_FOUND),
        (NOT_REJECTED, StatusCode::CONFLICT),
        (NOT_YOURS, StatusCode::FORBIDDEN),
    ] {
        let request = undo(id)
            .header("authorization", "Bearer alice")
            .body(Body::empty())
            .expect("request");
        assert_eq!(status(&app, request).await, expected, "{id}");
    }
}
