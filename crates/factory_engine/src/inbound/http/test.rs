use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use factory_records::domain::models::{HandBack, Outcome, RunId, RunInsert};
use factory_records::domain::ports::RunRepository;
use macro_authorization::{
    CredentialOrigin, INTERNAL_API_KEY_HEADER, INTERNAL_MACRO_USER_ID_HEADER, InternalAuthConfig,
    JwtValidator, MacroAuthorizationError, MacroAuthorizationServiceImpl, MacroAuthorizationState,
    NoBotAuthorizer, NoUserApiKeyAuthorizer, ValidatedCredential, ValidatedIdentity,
};
use macro_user_id::user_id::MacroUserIdStr;
use rootcause::Report;
use tower::ServiceExt as _;
use upstream_error::{UpstreamError, UpstreamFailure};
use uuid::Uuid;

use super::{FactoryRouterState, factory_router};
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::fakes::FakeRepo;
use crate::domain::gate::AnswerGate;
use crate::domain::models::{FactoryError, RunDetail, RunStart};
use crate::domain::ports::{AnswerDelivery, FactoryService, RunWorkflows, RunWorkflowsError};
use crate::domain::service::FactoryServiceImpl;
use crate::outbound::local_agent::LocalAgent;

const KEY: &str = "test-internal-key";
const ALICE: &str = "macro|alice@example.com";
const EVE: &str = "macro|eve@example.com";

/// Accepts the bearer tokens `alice` and `eve`, each its person's own
/// session.
#[derive(Clone, Copy)]
struct Tokens;

impl JwtValidator for Tokens {
    fn validate_credential(
        &self,
        jwt: &str,
    ) -> Result<ValidatedCredential, Report<MacroAuthorizationError>> {
        let user_id = match jwt {
            "alice" => ALICE,
            "eve" => EVE,
            _ => return Err(Report::new(MacroAuthorizationError::InvalidCredentials)),
        };
        Ok(ValidatedCredential {
            origin: CredentialOrigin::Session,
            identity: ValidatedIdentity {
                agent_id: None,
                agent_via: None,
                user_id: user_id.to_string(),
                fusion_user_id: format!("fusion-{jwt}"),
                organization_id: None,
                permissions: None,
            },
        })
    }
}

fn authorization() -> MacroAuthorizationState<MacroAuthorizationServiceImpl<Tokens>> {
    MacroAuthorizationState::new(Arc::new(MacroAuthorizationServiceImpl::new(
        Tokens,
        InternalAuthConfig {
            api_key: KEY.to_string(),
            default_user_id: None,
        },
        NoBotAuthorizer,
        NoUserApiKeyAuthorizer,
    )))
}

/// Who makes a call.
#[derive(Clone, Copy)]
enum Caller<'a> {
    /// No credential at all.
    Nobody,
    /// A person signed in with this bearer token.
    User(&'a str),
    /// An internal service, for this acting user when one is named.
    Internal(Option<&'a str>),
}

fn call(method: &str, path: &str, caller: Caller<'_>, body: serde_json::Value) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    match caller {
        Caller::Nobody => {}
        Caller::User(token) => {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        Caller::Internal(acting_user) => {
            request = request.header(INTERNAL_API_KEY_HEADER, KEY);
            if let Some(user) = acting_user {
                request = request.header(INTERNAL_MACRO_USER_ID_HEADER, user);
            }
        }
    }
    request.body(Body::from(body.to_string())).expect("request")
}

async fn body(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json")
}

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
    let state = FactoryRouterState::new(Arc::clone(&service), authorization());
    (factory_router(state), service)
}

/// Workflows that take every start, answer and cancel.
#[derive(Clone, Copy)]
struct AcceptingWorkflows;

impl RunWorkflows for AcceptingWorkflows {
    async fn start(&self, _start: &RunStart) -> Result<(), RunWorkflowsError> {
        Ok(())
    }

    async fn exists(&self, _run_id: RunId) -> Result<bool, RunWorkflowsError> {
        Ok(true)
    }

    async fn answer(
        &self,
        _run_id: RunId,
        _answer: &AnswerGate,
    ) -> Result<AnswerDelivery, RunWorkflowsError> {
        Ok(AnswerDelivery::Taken)
    }

    async fn cancel(&self, _run_id: RunId) -> Result<bool, RunWorkflowsError> {
        Ok(true)
    }
}

/// The Run API over the records in `repo`, as the service serves it.
fn records_router(repo: &FakeRepo) -> Router {
    let service = FactoryServiceImpl::new(
        repo.clone(),
        AcceptingWorkflows,
        BuiltInSteps::new(LocalAgent),
        repo.clone(),
        repo.clone(),
    );
    factory_router(FactoryRouterState::new(Arc::new(service), authorization()))
}

/// A Run of Alice's, kept with no task.
async fn alice_s_run(repo: &FakeRepo) -> RunId {
    let alice = MacroUserIdStr::try_from(ALICE.to_string()).expect("valid");
    match repo
        .insert_run(RunId::generate(), "Plan the export", &alice, None)
        .await
        .expect("insert")
    {
        RunInsert::Inserted(run) => run.id,
        RunInsert::NoSuchTask => panic!("a run with no task is kept"),
    }
}

#[tokio::test]
async fn a_call_for_no_one_is_unauthorized() {
    let (router, service) = router(|| FactoryError::NotFound);
    let run = format!("/runs/{}", RunId::generate());

    let anonymous = router
        .clone()
        .oneshot(call("GET", &run, Caller::Nobody, serde_json::Value::Null))
        .await
        .expect("response");
    let internal = router
        .oneshot(call(
            "GET",
            &run,
            Caller::Internal(None),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");

    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(internal.status(), StatusCode::UNAUTHORIZED);
    assert!(service.actors.lock().expect("actors").is_empty());
}

/// A person's call acts for them, from their own login; an internal call
/// acts for the user it names.
#[tokio::test]
async fn a_call_acts_for_the_signed_in_user_or_the_internal_caller_s_user() {
    let (router, service) = router(|| FactoryError::NotFound);
    let run = format!("/runs/{}", RunId::generate());

    let signed_in = router
        .clone()
        .oneshot(call(
            "GET",
            &run,
            Caller::User("alice"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    let internal = router
        .oneshot(call(
            "GET",
            &run,
            Caller::Internal(Some("macro|julia@example.com")),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");

    assert_eq!(signed_in.status(), StatusCode::NOT_FOUND);
    assert_eq!(internal.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        *service.actors.lock().expect("actors"),
        [ALICE, "macro|julia@example.com"]
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
            Caller::Internal(Some("macro|Julia@Example.com")),
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
            &format!("/runs/{}/cancel", RunId::generate()),
            Caller::User("alice"),
            serde_json::Value::Null,
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
                Caller::User("alice"),
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

/// A Run id says nothing to anyone but its owner: someone else reads,
/// answers and cancels it as missing.
#[tokio::test]
async fn someone_else_s_run_is_missing_to_them() {
    let repo = FakeRepo::default();
    let run = alice_s_run(&repo).await;
    let router = records_router(&repo);
    let send = |method: &'static str, path: String, token: &'static str, body| {
        let router = router.clone();
        async move {
            router
                .oneshot(call(method, &path, Caller::User(token), body))
                .await
                .expect("response")
                .status()
        }
    };

    let eve_reads = send(
        "GET",
        format!("/runs/{run}"),
        "eve",
        serde_json::Value::Null,
    )
    .await;
    let eve_answers = send(
        "POST",
        format!("/runs/{run}/steps/1/answer"),
        "eve",
        serde_json::json!({ "answer": "approve" }),
    )
    .await;
    let eve_cancels = send(
        "POST",
        format!("/runs/{run}/cancel"),
        "eve",
        serde_json::Value::Null,
    )
    .await;
    let alice_reads = send(
        "GET",
        format!("/runs/{run}"),
        "alice",
        serde_json::Value::Null,
    )
    .await;

    assert_eq!(eve_reads, StatusCode::NOT_FOUND);
    assert_eq!(eve_answers, StatusCode::NOT_FOUND);
    assert_eq!(eve_cancels, StatusCode::NOT_FOUND);
    assert_eq!(alice_reads, StatusCode::OK);
}

/// A Run is linked only to a task its owner can see; a hidden task reads
/// as no task at all.
#[tokio::test]
async fn a_run_is_not_linked_to_a_task_its_owner_cannot_see() {
    let repo = FakeRepo::default();
    let run = alice_s_run(&repo).await;
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let router = records_router(&repo);
    let link = || {
        call(
            "PUT",
            &format!("/runs/{run}/task"),
            Caller::User("alice"),
            serde_json::json!({ "taskId": task }),
        )
    };

    let seen = router.clone().oneshot(link()).await.expect("response");
    repo.hidden
        .lock()
        .expect("hidden")
        .insert((ALICE.to_string(), task));
    let hidden = router.oneshot(link()).await.expect("response");

    assert_eq!(seen.status(), StatusCode::OK);
    assert_eq!(hidden.status(), StatusCode::BAD_REQUEST);
}
