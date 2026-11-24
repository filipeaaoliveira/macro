use factory_records::domain::standing_rules::RunRules;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use bot_id::BotId;
use factory_records::domain::main_hold::{MainHoldRepository, RequiredChecksState};
use factory_records::domain::models::{
    EntryId, HandBack, NewRun, Outcome, Repository, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use macro_authorization::{
    AuthorizedUserApiKey, CODE_EXECUTION_HEADER, CredentialOrigin, INTERNAL_API_KEY_HEADER,
    INTERNAL_MACRO_USER_ID_HEADER, InternalAuthConfig, JwtValidator, MacroAuthorizationError,
    MacroAuthorizationServiceImpl, MacroAuthorizationState, NoBotAuthorizer, USER_API_KEY_HEADER,
    UserApiKeyAuthorizer, ValidatedCredential, ValidatedIdentity,
};
use macro_user_id::user_id::MacroUserIdStr;
use model_user::UserContext;
use rootcause::Report;
use tower::ServiceExt as _;
use upstream_error::{UpstreamError, UpstreamFailure};
use uuid::Uuid;

use super::{FactoryRouterState, factory_router};
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::fakes::{FakeGithub, FakeRepo, FakeWatches};
use crate::domain::gate::{AnswerGate, ReplyGate};
use crate::domain::models::{
    ActingAgent, Answerer, ControlFilesSettings, DefinitionSettings, FactoryError, FlowSummary,
    RunDetail, RunStart,
};
use crate::domain::ports::{AnswerDelivery, FactoryService, RunWorkflows, RunWorkflowsError};
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::service::FactoryServiceImpl;
use crate::outbound::local_agent::LocalAgent;

mod board;
mod control_files;
mod definition_changes;
mod flaws;
mod orchestrator;
mod standing_rules;
mod step_terminal;
mod waiting;

const KEY: &str = "test-internal-key";
const ALICE: &str = "macro|alice@example.com";
const EVE: &str = "macro|eve@example.com";
/// An agent's account: it signs in with a session as a person does.
const ROBOT: &str = "macro|robot@example.com";

/// Accepts the bearer tokens `alice` and `eve`, each its person's own
/// session, `robot`: an agent account's session, `sdk`: Alice's Macro API
/// token, `agent`: that token carrying an agent's attribution, as code mode
/// mints, `routine` and `routine-session`: a routine's token, and one that
/// reads as a session yet signs a routine, `routine-agent`: a routine's
/// token signed for an agent, and `mcp-client` and `mcp-key`: what public
/// MCP signs for Alice's own MCP client and API key, the Macro bot as
/// their label, with the client's name and without.
#[derive(Clone, Copy)]
struct Tokens;

impl JwtValidator for Tokens {
    fn validate_credential(
        &self,
        jwt: &str,
    ) -> Result<ValidatedCredential, Report<MacroAuthorizationError>> {
        let agent = BotId::new_from_uuid(Uuid::from_u128(0xB07A));
        let routine = Uuid::from_u128(0x2007);
        let macro_bot = Some(bot_id::MACRO_AI_BOT_ID);
        let (user_id, origin, agent_id, routine_id) = match jwt {
            "alice" => (ALICE, CredentialOrigin::Session, None, None),
            "eve" => (EVE, CredentialOrigin::Session, None, None),
            "robot" => (ROBOT, CredentialOrigin::Session, None, None),
            "sdk" => (ALICE, CredentialOrigin::MacroApiJwt, None, None),
            "agent" => (ALICE, CredentialOrigin::MacroApiJwt, Some(agent), None),
            "routine" => (ALICE, CredentialOrigin::MacroApiJwt, None, Some(routine)),
            "routine-session" => (ALICE, CredentialOrigin::Session, None, Some(routine)),
            "routine-agent" => (
                ALICE,
                CredentialOrigin::MacroApiJwt,
                Some(agent),
                Some(routine),
            ),
            "mcp-client" | "mcp-key" => (ALICE, CredentialOrigin::MacroApiJwt, macro_bot, None),
            _ => return Err(Report::new(MacroAuthorizationError::InvalidCredentials)),
        };
        Ok(ValidatedCredential {
            origin,
            identity: ValidatedIdentity {
                agent_id,
                agent_via: (jwt == "mcp-client").then(|| "Some client".to_string()),
                routine_id,
                user_id: user_id.to_string(),
                fusion_user_id: format!("fusion-{jwt}"),
                organization_id: None,
                permissions: None,
            },
        })
    }
}

/// Accepts the user API key `alice-key`, Alice's own.
#[derive(Clone, Copy)]
struct ApiKeys;

impl UserApiKeyAuthorizer for ApiKeys {
    async fn authorize_user_api_key(
        &self,
        api_key: &str,
    ) -> Result<AuthorizedUserApiKey, Report<MacroAuthorizationError>> {
        if api_key != "alice-key" {
            return Err(Report::new(MacroAuthorizationError::InvalidCredentials));
        }
        Ok(AuthorizedUserApiKey {
            user_context: UserContext {
                user_id: ALICE.to_string(),
                fusion_user_id: "fusion-alice".to_string(),
                permissions: None,
                organization_id: None,
            },
            account_manager: None,
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
        ApiKeys,
    )))
}

/// Who makes a call.
#[derive(Clone, Copy)]
enum Caller<'a> {
    /// No credential at all.
    Nobody,
    /// A person signed in with this bearer token.
    User(&'a str),
    /// A person's user API key.
    ApiKey(&'a str),
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
        Caller::ApiKey(key) => {
            request = request.header(USER_API_KEY_HEADER, key);
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
    answerers: Mutex<Vec<Answerer>>,
    /// The agent each start, cancel and link acted as, in order.
    acting: Mutex<Vec<Option<ActingAgent>>>,
    /// Whether each cancel came from the person in their own session.
    cancels_in_person: Mutex<Vec<bool>>,
}

impl FakeService {
    fn refusing(&self, actor: &MacroUserIdStr<'static>) -> FactoryError {
        self.actors.lock().expect("actors").push(actor.to_string());
        (self.answer)()
    }
}

impl FactoryService for FakeService {
    async fn list_flows(
        &self,
        _actor: &MacroUserIdStr<'static>,
    ) -> Result<Vec<FlowSummary>, FactoryError> {
        Ok(Vec::new())
    }

    async fn create_run(
        &self,
        actor: MacroUserIdStr<'static>,
        _flow: String,
        _request: String,
        _task_id: Option<Uuid>,
        _repo_url: Option<String>,
        _in_person: bool,
        acting: Option<ActingAgent>,
    ) -> Result<RunDetail, FactoryError> {
        self.acting.lock().expect("acting").push(acting);
        Err(self.refusing(&actor))
    }

    async fn get_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        _id: RunId,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn list_task_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
        _task_id: Uuid,
    ) -> Result<Vec<RunDetail>, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn list_waiting_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<Vec<RunDetail>, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn list_starting_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<Vec<RunDetail>, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn board(
        &self,
        actor: &MacroUserIdStr<'static>,
        _filter: crate::domain::board::BoardFilter,
    ) -> Result<crate::domain::board::BoardPage, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn pull_request_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
        _urls: Vec<String>,
    ) -> Result<crate::domain::board::PullRequestRuns, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn link_task(
        &self,
        actor: &MacroUserIdStr<'static>,
        _id: RunId,
        _task_id: Uuid,
        acting: Option<ActingAgent>,
    ) -> Result<RunDetail, FactoryError> {
        self.acting.lock().expect("acting").push(acting);
        Err(self.refusing(actor))
    }

    async fn answer_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        _id: RunId,
        _seq: u16,
        _answer: Outcome,
        _note: Option<String>,
        _approved_control_files: Option<Vec<String>>,
        answerer: Answerer,
    ) -> Result<(), FactoryError> {
        self.answerers.lock().expect("answerers").push(answerer);
        Err(self.refusing(&actor))
    }

    async fn reply_to_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        _id: RunId,
        _seq: u16,
        _reply: String,
    ) -> Result<(), FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn session_step(
        &self,
        actor: &MacroUserIdStr<'static>,
        _session_id: Uuid,
    ) -> Result<crate::domain::models::SessionStepRef, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn steer_step(
        &self,
        actor: MacroUserIdStr<'static>,
        _id: RunId,
        _seq: u16,
        _request: crate::domain::step_terminal::TerminalRequest,
    ) -> Result<(), FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn control_files(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<ControlFilesSettings, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn definition(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<DefinitionSettings, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn set_definition(
        &self,
        actor: MacroUserIdStr<'static>,
        _repo_url: Option<String>,
    ) -> Result<DefinitionSettings, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn propose_definition_change(
        &self,
        actor: MacroUserIdStr<'static>,
        _change_id: Uuid,
        _change: crate::domain::definition::ProposedChange,
    ) -> Result<crate::domain::models::DefinitionPullRequest, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn definition_file(
        &self,
        actor: &MacroUserIdStr<'static>,
        _path: String,
    ) -> Result<crate::domain::models::DefinitionFile, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn check_definition_change(
        &self,
        actor: MacroUserIdStr<'static>,
        _change: crate::domain::definition::ProposedChange,
    ) -> Result<crate::domain::models::DefinitionChangeCheck, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn set_control_files(
        &self,
        actor: MacroUserIdStr<'static>,
        _patterns: Option<Vec<String>>,
    ) -> Result<ControlFilesSettings, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn standing_rules(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<crate::domain::models::StandingRulesSettings, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn set_standing_rules(
        &self,
        actor: MacroUserIdStr<'static>,
        _definition: Option<factory_records::domain::standing_rules::OrchestratorDefinition>,
    ) -> Result<crate::domain::models::StandingRulesSettings, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn set_project_rules(
        &self,
        actor: MacroUserIdStr<'static>,
        _project_id: Uuid,
        _definition: Option<factory_records::domain::standing_rules::OrchestratorDefinition>,
    ) -> Result<crate::domain::models::StandingRulesSettings, FactoryError> {
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
        acting: Option<ActingAgent>,
        in_person: bool,
    ) -> Result<(), FactoryError> {
        self.acting.lock().expect("acting").push(acting);
        self.cancels_in_person
            .lock()
            .expect("cancels in person")
            .push(in_person);
        Err(self.refusing(actor))
    }

    async fn retry_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        _id: RunId,
        acting: Option<ActingAgent>,
        _in_person: bool,
    ) -> Result<RunDetail, FactoryError> {
        self.acting.lock().expect("acting").push(acting);
        Err(self.refusing(actor))
    }

    async fn mark_run_flawed(
        &self,
        actor: &MacroUserIdStr<'static>,
        _id: RunId,
        _reason: &str,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn check_run_order(
        &self,
        actor: &MacroUserIdStr<'static>,
        _flow: &str,
        _repo_url: Option<&str>,
    ) -> Result<(), FactoryError> {
        Err(self.refusing(actor))
    }

    async fn create_automation_run(
        &self,
        automation: &factory_records::domain::automations::Automation,
        _firing_key: &str,
        _request: String,
        _task_id: Option<Uuid>,
        _acting: Option<ActingAgent>,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(&automation.created_by))
    }

    async fn benchmark_base(
        &self,
        actor: &MacroUserIdStr<'static>,
        _repo_url: &str,
    ) -> Result<String, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn team_evaluation(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<Option<crate::domain::evaluation::TeamEvaluation>, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn create_trial_run(
        &self,
        actor: MacroUserIdStr<'static>,
        _flow: String,
        _request: String,
        _repo_url: String,
        _trial: factory_records::domain::evaluation::TrialKey,
        _base_commit: String,
        _started_by: Option<factory_records::domain::standing_acts::StandingActor>,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn standing_start(
        &self,
        actor: &MacroUserIdStr<'static>,
        _agents: Option<&crate::domain::agents::AgentDefinition>,
        _acting: Option<ActingAgent>,
    ) -> Result<Option<factory_records::domain::standing_acts::StandingActor>, FactoryError> {
        Err(self.refusing(actor))
    }

    async fn create_improvement_run(
        &self,
        actor: MacroUserIdStr<'static>,
        _request: String,
        _repo_url: String,
        _improvement: Uuid,
    ) -> Result<RunDetail, FactoryError> {
        Err(self.refusing(&actor))
    }

    async fn clear_main_hold(
        &self,
        _repo_url: &str,
        _base_branch: &str,
    ) -> Result<bool, FactoryError> {
        Ok(false)
    }
}

fn router(answer: fn() -> FactoryError) -> (Router, Arc<FakeService>) {
    let service = Arc::new(FakeService {
        answer,
        actors: Mutex::new(Vec::new()),
        answerers: Mutex::new(Vec::new()),
        acting: Mutex::new(Vec::new()),
        cancels_in_person: Mutex::new(Vec::new()),
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

    async fn reply(
        &self,
        _run_id: RunId,
        _reply: &ReplyGate,
    ) -> Result<AnswerDelivery, RunWorkflowsError> {
        Ok(AnswerDelivery::Taken)
    }

    async fn cancel(&self, _run_id: RunId) -> Result<bool, RunWorkflowsError> {
        Ok(true)
    }
}

/// The Run API over the records in `repo`, as the service serves it.
fn records_router(repo: &FakeRepo) -> Router {
    github_router(repo, FakeGithub::default())
}

/// The Run API over `repo`, acting on GitHub through `github`.
fn github_router(repo: &FakeRepo, github: FakeGithub) -> Router {
    let service = FactoryServiceImpl::new(
        repo.clone(),
        AcceptingWorkflows,
        BuiltInSteps::new(
            LocalAgent,
            PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
        ),
        repo.clone(),
        repo.clone(),
        github,
    );
    factory_router(FactoryRouterState::new(Arc::new(service), authorization()))
}

/// A Run of Alice's, kept with no task.
async fn alice_s_run(repo: &FakeRepo) -> RunId {
    let alice = MacroUserIdStr::try_from(ALICE.to_string()).expect("valid");
    match repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Plan the export",
            user_id: &alice,
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    {
        RunInsert::Inserted(run) => run.id,
        RunInsert::NoSuchTask
        | RunInsert::TaskBusy
        | RunInsert::FiringStarted
        | RunInsert::QueueFull(_) => {
            panic!("a run with no task is kept")
        }
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
            serde_json::json!({ "flow": "reply", "request": "Fix the login bug" }),
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

/// Only a Run's owner retries it, and only once it ended: someone else's
/// Run is missing to them, and a running one, or one retried already, is a
/// conflict.
#[tokio::test]
async fn a_retry_reaches_only_its_owner_s_ended_run() {
    let repo = FakeRepo::default();
    let run = alice_s_run(&repo).await;
    let records = records_router(&repo);
    let retry = |router: Router, token: &'static str| async move {
        router
            .oneshot(call(
                "POST",
                &format!("/runs/{run}/retry"),
                Caller::User(token),
                serde_json::Value::Null,
            ))
            .await
            .expect("response")
            .status()
    };

    let eve_retries = retry(records.clone(), "eve").await;
    let alice_retries_a_running_run = retry(records, "alice").await;
    let retried_again = retry(router(|| FactoryError::AlreadyRetried).0, "alice").await;

    assert_eq!(eve_retries, StatusCode::NOT_FOUND);
    assert_eq!(alice_retries_a_running_run, StatusCode::CONFLICT);
    assert_eq!(retried_again, StatusCode::CONFLICT);
    assert!(repo.runs.lock().expect("runs").len() == 1);
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

/// The Flows a person can start, with what each does and whether it works
/// on a repository: every built-in one, as the worker serves all their Steps.
#[tokio::test]
async fn a_signed_in_user_lists_the_flows_the_worker_runs() {
    let router = records_router(&FakeRepo::default());

    let listed = router
        .clone()
        .oneshot(call(
            "GET",
            "/flows",
            Caller::User("alice"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    let anonymous = router
        .oneshot(call(
            "GET",
            "/flows",
            Caller::Nobody,
            serde_json::Value::Null,
        ))
        .await
        .expect("response");

    assert_eq!(listed.status(), StatusCode::OK);
    let flows = body(listed).await["flows"].clone();
    let names: Vec<&str> = flows
        .as_array()
        .expect("flows")
        .iter()
        .map(|flow| flow["name"].as_str().expect("name"))
        .collect();
    assert_eq!(names, ["code", "plan", "reply"]);
    assert!(
        flows[0]["description"]
            .as_str()
            .expect("description")
            .starts_with("Takes a task to a merged pull request")
    );
    let on_repository: Vec<bool> = flows
        .as_array()
        .expect("flows")
        .iter()
        .map(|flow| flow["repository"].as_bool().expect("repository"))
        .collect();
    assert_eq!(on_repository, [true, false, false]);
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
}

/// A person starts a Run of a built-in Flow by its name, on a task they can
/// see, and owns it.
#[tokio::test]
async fn a_signed_in_user_starts_a_run_of_a_flow_by_its_name() {
    let repo = FakeRepo::default();
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);

    let response = records_router(&repo)
        .oneshot(call(
            "POST",
            "/runs",
            Caller::User("alice"),
            serde_json::json!({ "flow": "plan", "request": "Add a due date column", "taskId": task }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::CREATED);
    let created = body(response).await;
    assert_eq!(created["run"]["userId"], ALICE);
    assert_eq!(created["run"]["taskId"], task.to_string());
    assert_eq!(created["steps"], serde_json::json!([]));
    assert_eq!(repo.runs.lock().expect("runs").len(), 1);
}

/// A `code` Run starts on the repository its request names, and reads it
/// back with the branch it starts from.
#[tokio::test]
async fn a_code_run_starts_on_the_repository_it_names() {
    let repo = FakeRepo::default();

    let response = records_router(&repo)
        .oneshot(call(
            "POST",
            "/runs",
            Caller::User("alice"),
            serde_json::json!({
                "flow": "code",
                "request": "Collapse repeated dashes",
                "repoUrl": "https://github.com/macro/app"
            }),
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::CREATED);
    let created = body(response).await;
    assert_eq!(
        created["run"]["repository"],
        serde_json::json!({ "url": "https://github.com/macro/app", "baseBranch": "main" })
    );
}

/// A `code` Run refused because the caller's GitHub account cannot reach
/// the repository, or cannot push to it, says which, for the client to
/// name the cause; other refusals carry no code.
#[tokio::test]
async fn a_refused_repository_says_why() {
    let start = |github: FakeGithub, flow: &str| {
        let repo = FakeRepo::default();
        let request = serde_json::json!({
            "flow": flow,
            "request": "Collapse repeated dashes",
            "repoUrl": "https://github.com/macro/app"
        });
        async move {
            let response = github_router(&repo, github)
                .oneshot(call("POST", "/runs", Caller::User("alice"), request))
                .await
                .expect("response");
            (response.status(), body(response).await)
        }
    };
    let unreachable = FakeGithub::default();
    unreachable.state().unreachable = true;
    let read_only = FakeGithub::default();
    read_only.state().read_only = true;

    let (unreachable_status, unreachable) = start(unreachable, "code").await;
    let (read_only_status, read_only) = start(read_only, "code").await;
    let (not_used_status, not_used) = start(FakeGithub::default(), "reply").await;

    assert_eq!(unreachable_status, StatusCode::BAD_REQUEST);
    assert_eq!(unreachable["code"], "unreachableRepository");
    assert_eq!(read_only_status, StatusCode::BAD_REQUEST);
    assert_eq!(read_only["code"], "noPushAccess");
    assert_eq!(not_used_status, StatusCode::BAD_REQUEST);
    assert_eq!(not_used.get("code"), None);
    assert!(not_used["message"].is_string());
}

/// Only a built-in Flow starts: a whole definition, or a name no Flow has,
/// is refused, and so is a task the caller cannot see. Nothing is kept.
#[tokio::test]
async fn a_run_starts_only_of_a_built_in_flow_on_a_task_the_caller_sees() {
    let repo = FakeRepo::default();
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    repo.hidden
        .lock()
        .expect("hidden")
        .insert((EVE.to_string(), task));
    let router = records_router(&repo);
    let start = |token: &'static str, body: serde_json::Value| {
        let router = router.clone();
        async move {
            router
                .oneshot(call("POST", "/runs", Caller::User(token), body))
                .await
                .expect("response")
                .status()
        }
    };

    let definition = start(
        "alice",
        serde_json::json!({
            "flow": { "entries": [{ "id": "reply", "step": "reply" }] },
            "request": "Say hello",
        }),
    )
    .await;
    let unknown = start(
        "alice",
        serde_json::json!({ "flow": "deploy", "request": "Ship it" }),
    )
    .await;
    let hidden_task = start(
        "eve",
        serde_json::json!({ "flow": "plan", "request": "Plan it", "taskId": task }),
    )
    .await;

    assert!(definition.is_client_error(), "{definition}");
    assert_eq!(unknown, StatusCode::BAD_REQUEST);
    assert_eq!(hidden_task, StatusCode::BAD_REQUEST);
    assert!(repo.runs.lock().expect("runs").is_empty());
}

/// Each person lists their own Runs on a task, newest first, each with its
/// Steps; someone else's Runs on the same task are not theirs to see.
#[tokio::test]
async fn a_task_s_runs_are_the_caller_s_own_newest_first() {
    let repo = FakeRepo::default();
    let task = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    let router = records_router(&repo);
    let start = |token: &'static str, request: &'static str| {
        let router = router.clone();
        async move {
            let response = router
                .oneshot(call(
                    "POST",
                    "/runs",
                    Caller::User(token),
                    serde_json::json!({ "flow": "plan", "request": request, "taskId": task }),
                ))
                .await
                .expect("response");
            body(response).await["run"]["id"].clone()
        }
    };
    let first = start("alice", "Plan the export").await;
    let theirs = start("eve", "Plan the import").await;
    let second = start("alice", "Plan the export again").await;
    let first_id: RunId = serde_json::from_value(first.clone()).expect("run id");
    repo.start_step(
        StepKey {
            run_id: first_id,
            seq: 1,
        },
        &EntryId::try_from("plan".to_string()).expect("entry"),
        1,
    )
    .await
    .expect("start");
    let list = |token: &'static str| {
        let router = router.clone();
        async move {
            router
                .oneshot(call(
                    "GET",
                    &format!("/tasks/{task}/runs"),
                    Caller::User(token),
                    serde_json::Value::Null,
                ))
                .await
                .expect("response")
        }
    };

    let alice_s = list("alice").await;
    let eve_s = list("eve").await;

    assert_eq!(alice_s.status(), StatusCode::OK);
    let runs = body(alice_s).await["runs"].clone();
    let ids: Vec<serde_json::Value> = runs
        .as_array()
        .expect("runs")
        .iter()
        .map(|detail| detail["run"]["id"].clone())
        .collect();
    assert_eq!(ids, [second, first]);
    assert_eq!(runs[0]["steps"], serde_json::json!([]));
    assert_eq!(runs[1]["steps"][0]["entry"], "plan");
    let runs = body(eve_s).await["runs"].clone();
    assert_eq!(runs.as_array().expect("runs").len(), 1);
    assert_eq!(runs[0]["run"]["id"], theirs);
}

/// A task the caller cannot see, one that does not exist, and a document
/// that is not a task are all missing to them; a caller with no login is
/// refused before that.
#[tokio::test]
async fn a_task_the_caller_cannot_see_has_no_runs_to_list() {
    let repo = FakeRepo::default();
    let task = Uuid::now_v7();
    let note = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task);
    repo.documents.lock().expect("documents").insert(note);
    repo.hidden
        .lock()
        .expect("hidden")
        .insert((EVE.to_string(), task));
    let router = records_router(&repo);
    let list = |caller: Caller<'static>, task: Uuid| {
        let router = router.clone();
        async move {
            router
                .oneshot(call(
                    "GET",
                    &format!("/tasks/{task}/runs"),
                    caller,
                    serde_json::Value::Null,
                ))
                .await
                .expect("response")
                .status()
        }
    };

    let hidden = list(Caller::User("eve"), task).await;
    let missing = list(Caller::User("alice"), Uuid::now_v7()).await;
    let not_a_task = list(Caller::User("alice"), note).await;
    let anonymous = list(Caller::Nobody, task).await;
    let seen = list(Caller::User("alice"), task).await;

    assert_eq!(hidden, StatusCode::NOT_FOUND);
    assert_eq!(missing, StatusCode::NOT_FOUND);
    assert_eq!(not_a_task, StatusCode::NOT_FOUND);
    assert_eq!(anonymous, StatusCode::UNAUTHORIZED);
    assert_eq!(seen, StatusCode::OK);
}

/// A Gate asks a person: only the owner in their own signed-in session
/// answers it. An internal caller acting for them, their user API key or
/// Macro API token, and an agent working as them, by its token's
/// attribution or a code-mode run's header, are refused before the Run is
/// looked at.
#[tokio::test]
async fn only_the_owner_s_own_session_answers_a_gate() {
    let (router, service) = router(|| FactoryError::NotFound);
    let path = format!("/runs/{}/steps/1/answer", RunId::generate());
    let answer = serde_json::json!({ "answer": "approve" });
    let send = |request: Request<Body>| {
        let router = router.clone();
        async move { router.oneshot(request).await.expect("response").status() }
    };

    let internal = send(call(
        "POST",
        &path,
        Caller::Internal(Some(ALICE)),
        answer.clone(),
    ))
    .await;
    let api_key = send(call(
        "POST",
        &path,
        Caller::ApiKey("alice-key"),
        answer.clone(),
    ))
    .await;
    let sdk = send(call("POST", &path, Caller::User("sdk"), answer.clone())).await;
    let attributed = send(call("POST", &path, Caller::User("agent"), answer.clone())).await;
    let mut in_code_mode = call("POST", &path, Caller::User("alice"), answer.clone());
    in_code_mode
        .headers_mut()
        .insert(CODE_EXECUTION_HEADER, "true".parse().expect("header"));
    let in_code_mode = send(in_code_mode).await;
    let person = send(call("POST", &path, Caller::User("alice"), answer)).await;

    assert_eq!(internal, StatusCode::FORBIDDEN);
    assert_eq!(api_key, StatusCode::FORBIDDEN);
    assert_eq!(sdk, StatusCode::FORBIDDEN);
    assert_eq!(attributed, StatusCode::FORBIDDEN);
    assert_eq!(in_code_mode, StatusCode::FORBIDDEN);
    assert_eq!(person, StatusCode::NOT_FOUND);
    assert_eq!(*service.actors.lock().expect("actors"), [ALICE]);
}

/// A person's user API key reaches the Run API as them, as their login
/// does.
#[tokio::test]
async fn a_user_api_key_acts_for_its_owner() {
    let (router, service) = router(|| FactoryError::NotFound);

    let response = router
        .oneshot(call(
            "GET",
            &format!("/runs/{}", RunId::generate()),
            Caller::ApiKey("alice-key"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(*service.actors.lock().expect("actors"), [ALICE]);
}

/// A Run at its merge on a held base shows the hold, which its merge waits
/// on; one that ended, is not at its merge yet, or works on another base
/// shows none. A base whose required checks could not be read says so.
#[tokio::test]
async fn a_run_at_its_merge_on_a_held_base_shows_the_main_hold() {
    let repo = FakeRepo::default();
    let alice = MacroUserIdStr::try_from(ALICE.to_string()).expect("valid");
    let main = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    let release = Repository {
        base_branch: "release".to_string(),
        ..main.clone()
    };
    let mut runs = Vec::new();
    for (base, entry) in [
        (&main, "merge-gate"),
        (&main, "merge-gate"),
        (&release, "merge"),
        (&main, "ci"),
    ] {
        let run_id = RunId::generate();
        repo.insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Add dark mode",
            user_id: &alice,
            task_id: None,
            project_id: None,
            repository: Some(base),
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert");
        let entry = EntryId::try_from(entry.to_string()).expect("entry");
        repo.start_step(StepKey { run_id, seq: 1 }, &entry, 1)
            .await
            .expect("start");
        runs.push(run_id);
    }
    repo.end_run(runs[1], RunOutcome::Manual)
        .await
        .expect("end");
    repo.hold_main(
        &main,
        "2222222222222222222222222222222222222222",
        "- test (failure): https://ci/1",
        chrono::Utc::now(),
    )
    .await
    .expect("hold");
    repo.mark_required(&release, RequiredChecksState::Unread)
        .await
        .expect("unread");

    let read = |run_id: RunId| {
        let router = records_router(&repo);
        async move {
            let response = router
                .oneshot(call(
                    "GET",
                    &format!("/runs/{run_id}"),
                    Caller::User("alice"),
                    serde_json::Value::Null,
                ))
                .await
                .expect("response");
            assert_eq!(response.status(), StatusCode::OK);
            body(response).await
        }
    };

    let held = read(runs[0]).await;
    assert_eq!(
        held["mainHold"]["headSha"],
        "2222222222222222222222222222222222222222"
    );
    assert_eq!(held["mainHold"]["failed"], "- test (failure): https://ci/1");
    assert!(held["mainHold"]["since"].is_string());
    assert!(held["mainHold"]["taskId"].is_null());
    assert!(held["mainHold"].get("taskClaimed").is_none());
    assert!(held["baseRequiredChecks"].is_null());
    assert!(read(runs[1]).await["mainHold"].is_null());
    let unread = read(runs[2]).await;
    assert!(unread["mainHold"].is_null());
    assert_eq!(unread["baseRequiredChecks"], "unread");
    assert!(read(runs[3]).await["mainHold"].is_null());
}

/// Only an internal caller clears a Main Hold: a person, even the owner of
/// Runs on the base, is refused.
#[tokio::test]
async fn only_an_internal_caller_clears_a_main_hold() {
    let repo = FakeRepo::default();
    let main = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    repo.hold_main(
        &main,
        "2222222222222222222222222222222222222222",
        "- test (failure)",
        chrono::Utc::now() - chrono::TimeDelta::minutes(1),
    )
    .await
    .expect("hold");
    let clear = serde_json::json!({
        "repoUrl": "https://github.com/macro/app",
        "baseBranch": "main",
    });

    let refused = records_router(&repo)
        .oneshot(call(
            "POST",
            "/internal/main-holds/clear",
            Caller::User("alice"),
            clear.clone(),
        ))
        .await
        .expect("response");
    assert!(refused.status().is_client_error(), "{}", refused.status());
    assert!(repo.main_hold(&main).await.expect("read").is_some());

    let cleared = records_router(&repo)
        .oneshot(call(
            "POST",
            "/internal/main-holds/clear",
            Caller::Internal(None),
            clear,
        ))
        .await
        .expect("response");
    assert_eq!(cleared.status(), StatusCode::OK);
    assert_eq!(body(cleared).await["held"], true);
    assert_eq!(repo.main_hold(&main).await.expect("read"), None);
}

/// A Definition's file is read for the caller, by the path the query
/// names; a change is checked only for a person in their own session, as
/// it is proposed, and its refusal carries the same code.
#[tokio::test]
async fn a_definition_file_and_a_change_check_reach_the_service() {
    let (router, service) = router(|| FactoryError::NoDefinition);

    let file = router
        .clone()
        .oneshot(call(
            "GET",
            "/settings/definition/file?path=flows%2Fcode.json",
            Caller::User("alice"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");
    let change = serde_json::json!({
        "title": "Say more",
        "files": { "flows/code.json": "{}" },
    });
    let internal = router
        .clone()
        .oneshot(call(
            "POST",
            "/settings/definition/changes/check",
            Caller::Internal(Some(ALICE)),
            change.clone(),
        ))
        .await
        .expect("response");
    let checked = router
        .oneshot(call(
            "POST",
            "/settings/definition/changes/check",
            Caller::User("alice"),
            change,
        ))
        .await
        .expect("response");

    assert_eq!(file.status(), StatusCode::CONFLICT);
    assert_eq!(body(file).await["code"], "noDefinition");
    assert_eq!(internal.status(), StatusCode::FORBIDDEN);
    assert_eq!(checked.status(), StatusCode::CONFLICT);
    assert_eq!(body(checked).await["code"], "noDefinition");
    assert_eq!(*service.actors.lock().expect("actors"), [ALICE, ALICE]);
}

/// A path that names no Definition file is refused with its own code.
#[tokio::test]
async fn a_path_that_is_no_definition_file_says_so() {
    let (router, _) = router(|| FactoryError::NotADefinitionFile("README.md".to_string()));

    let response = router
        .oneshot(call(
            "GET",
            "/settings/definition/file?path=README.md",
            Caller::User("alice"),
            serde_json::Value::Null,
        ))
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body(response).await["code"], "notADefinitionFile");
}
