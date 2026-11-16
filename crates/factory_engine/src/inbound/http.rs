//! Axum router for the Run API.

#[cfg(test)]
mod test;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
};
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, HandBack, Outcome, Run, RunEnd, RunId, RunOutcome, Step, StepEnd,
};
use macro_authorization::{
    MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState, UserOnly,
    UserOrInternal, is_agent_run,
};
use macro_user_id::user_id::MacroUserIdStr;
use model_error_response::ErrorResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::{FactoryError, FlowSummary, RunDetail};
use crate::domain::ports::FactoryService;

/// Router state for the Run API.
pub struct FactoryRouterState<S, Auth> {
    service: Arc<S>,
    authorization_state: MacroAuthorizationState<Auth>,
}

impl<S, Auth> Clone for FactoryRouterState<S, Auth> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            authorization_state: self.authorization_state.clone(),
        }
    }
}

impl<S, Auth> FactoryRouterState<S, Auth>
where
    S: FactoryService,
{
    /// Create router state from a shared service and authorization state.
    pub fn new(service: Arc<S>, authorization_state: MacroAuthorizationState<Auth>) -> Self {
        Self {
            service,
            authorization_state,
        }
    }
}

impl<S, Auth> FromRef<FactoryRouterState<S, Auth>> for MacroAuthorizationState<Auth> {
    fn from_ref(state: &FactoryRouterState<S, Auth>) -> Self {
        state.authorization_state.clone()
    }
}

/// Build the Run API router. People call it with their own login, and
/// internal callers for an acting user (the identity headers of the internal
/// key); either way the call reaches only that user's Runs. `POST
/// /hand-backs` is an agent session's: its code-mode SDK calls it as the
/// Run's owner.
///
/// Routes:
/// - `GET /flows` — the built-in Flows a Run can be started of.
/// - `POST /runs` — create a Run of a built-in Flow, owned by the acting
///   user, and start it.
/// - `GET /tasks/{task_id}/runs` — the caller's Runs on a task, newest
///   first.
/// - `GET /runs/{id}` — read a Run and the Steps it took.
/// - `POST /runs/{id}/steps/{seq}/answer` — answer the Gate of the Run's
///   Step `seq`.
/// - `POST /runs/{id}/cancel` — cancel a Run.
/// - `PUT /runs/{id}/task` — link a Run to the Macro task it works on.
/// - `POST /hand-backs` — an agent Step's session hands back its result.
pub fn factory_router<S, Auth, T>(state: FactoryRouterState<S, Auth>) -> Router<T>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
    T: Send + Sync + 'static,
{
    Router::new()
        .route("/flows", get(list_flows_handler::<S, Auth>))
        .route("/runs", post(create_run_handler::<S, Auth>))
        .route(
            "/tasks/{task_id}/runs",
            get(list_task_runs_handler::<S, Auth>),
        )
        .route("/runs/{id}", get(get_run_handler::<S, Auth>))
        .route(
            "/runs/{id}/steps/{seq}/answer",
            post(answer_gate_handler::<S, Auth>),
        )
        .route("/runs/{id}/cancel", post(cancel_run_handler::<S, Auth>))
        .route("/runs/{id}/task", put(link_task_handler::<S, Auth>))
        .route("/hand-backs", post(hand_back_handler::<S, Auth>))
        .with_state(state)
}

/// The Flows a Run can be started of.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FlowsResponse {
    /// The Flows, by name.
    pub flows: Vec<FlowSummary>,
}

/// Request body for creating a Run.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunRequest {
    /// The name of the built-in Flow the Run walks, one `GET /flows` lists.
    pub flow: String,
    /// What the Run is asked to do.
    pub request: String,
    /// The Macro task the Run works on: its task document's id.
    #[serde(default)]
    pub task_id: Option<Uuid>,
}

/// Request body for linking a Run to a task.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkTaskRequest {
    /// The task document's id.
    pub task_id: Uuid,
}

/// Path params naming a task.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct TaskRunsParams {
    /// The task document's id.
    pub task_id: Uuid,
}

/// A task's Runs, as the caller sees them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TaskRunsResponse {
    /// The caller's Runs on the task, newest first: at most the newest 50.
    pub runs: Vec<RunDetail>,
}

/// Path params naming a Run.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct GetRunParams {
    /// The Run's id.
    pub id: RunId,
}

/// Path params for answering a Gate.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct AnswerGateParams {
    /// The Run's id.
    pub id: RunId,
    /// The place, among the Run's Steps, of the Step whose Gate it answers.
    pub seq: u16,
}

/// Request body for answering a Gate.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnswerGateRequest {
    /// The answer, one the Gate offers.
    pub answer: Outcome,
    /// What the person adds, for the Run's later Steps to read.
    #[serde(default)]
    pub note: Option<String>,
}

/// Request body for handing back an agent Step's result.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandBackRequest {
    /// How the Step ended: one of the outcomes it offers.
    pub outcome: Outcome,
    /// What the agent produced, for the Run's later Steps and for people.
    pub text: String,
}

/// List the built-in Flows a Run can be started of, by name.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "list_factory_flows",
    path = "/flows",
    responses(
        (status = 200, body = FlowsResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn list_flows_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    _caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
) -> Json<FlowsResponse>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    Json(FlowsResponse {
        flows: state.service.list_flows(),
    })
}

/// Create a Run of a built-in Flow, owned by the caller, and start its
/// workflow.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "create_factory_run",
    path = "/runs",
    request_body = CreateRunRequest,
    responses(
        (status = 201, body = RunDetail),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn create_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Json(body): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<RunDetail>), FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    // The id is lowercase, whatever case the caller named it in: a
    // `MacroUserIdStr` lowercases what it parses, as `"User".id` is stored.
    let actor = caller.authorization.user.macro_user_id;
    let created = state
        .service
        .create_run(actor, body.flow, body.request, body.task_id)
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// List the caller's Runs on a task they can see, newest first, each with
/// the Steps it took. Other people's Runs on the task are not listed.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "list_factory_task_runs",
    path = "/tasks/{task_id}/runs",
    params(TaskRunsParams),
    responses(
        (status = 200, body = TaskRunsResponse),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(task_id = %params.task_id))]
pub async fn list_task_runs_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<TaskRunsParams>,
) -> Result<Json<TaskRunsResponse>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    let runs = state.service.list_task_runs(&actor, params.task_id).await?;
    Ok(Json(TaskRunsResponse { runs }))
}

/// Read one of the caller's Runs and the Steps it took.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_run",
    path = "/runs/{id}",
    params(GetRunParams),
    responses(
        (status = 200, body = RunDetail),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn get_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<GetRunParams>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(state.service.get_run(&actor, params.id).await?))
}

/// Answer, as the caller, the Gate one of their Runs waits on. The Run
/// goes on once its workflow took the answer; reading the Run shows the
/// answer once it is recorded. A Gate asks a person: an agent's program
/// acting for the owner is refused (403).
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "answer_factory_gate",
    path = "/runs/{id}/steps/{seq}/answer",
    params(AnswerGateParams),
    request_body = AnswerGateRequest,
    responses(
        (status = 202),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id, seq = params.seq))]
pub async fn answer_gate_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AnswerGateParams>,
    Json(body): Json<AnswerGateRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    // An agent works as the Run's owner, so its code could otherwise
    // approve the plan it wrote.
    let user = caller.authorization.user;
    if is_agent_run(user.agent_id.is_some(), &headers) {
        return Err(FactoryError::AgentAnswer);
    }
    let actor = user.macro_user_id;
    state
        .service
        .answer_gate(actor, params.id, params.seq, body.answer, body.note)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// Cancel one of the caller's Runs. Its running Step or Gate stops, and
/// reading the Run shows it `cancelled` once its workflow recorded that.
/// Cancelling a cancelled Run again is accepted and changes nothing.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "cancel_factory_run",
    path = "/runs/{id}/cancel",
    params(GetRunParams),
    responses(
        (status = 202),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn cancel_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<GetRunParams>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    state.service.cancel_run(&actor, params.id).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Link one of the caller's Runs to a Macro task they can see,
/// replacing any task it had.
#[utoipa::path(
    put,
    tag = "factory",
    operation_id = "link_factory_run_task",
    path = "/runs/{id}/task",
    params(GetRunParams),
    request_body = LinkTaskRequest,
    responses(
        (status = 200, body = RunDetail),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn link_task_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<GetRunParams>,
    Json(body): Json<LinkTaskRequest>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(
        state
            .service
            .link_task(&actor, params.id, body.task_id)
            .await?,
    ))
}

/// Hand back the result of the factory Step the calling agent session works
/// on. The session's latest hand back is the Step's result once its turn
/// ends.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "hand_back_factory_step",
    path = "/hand-backs",
    request_body = HandBackRequest,
    responses(
        (status = 204),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn hand_back_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOnly>,
    Json(body): Json<HandBackRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    // The session behind the code-mode credential, validated against it by
    // the correlation layer; no other caller names one.
    let session_id =
        agent_correlation::context::current_session().ok_or(FactoryError::NotAStepSession)?;
    let hand_back = HandBack {
        outcome: body.outcome,
        output: body.text,
    };
    state
        .service
        .hand_back(session_id, caller.authorization.macro_user_id, hand_back)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

impl FactoryError {
    /// The status the API answers this error with.
    fn status(&self) -> StatusCode {
        match self {
            FactoryError::InvalidFlow(_)
            | FactoryError::EmptyRequest
            | FactoryError::RequestTooLarge { .. }
            | FactoryError::NoteTooLarge { .. }
            | FactoryError::UnknownTask
            | FactoryError::NotATask
            | FactoryError::UnknownUser
            | FactoryError::UnknownFlow
            | FactoryError::AnswerNotOffered { .. }
            | FactoryError::NotAStepSession
            | FactoryError::OutcomeNotOffered { .. }
            | FactoryError::HandBackTooLarge { .. } => StatusCode::BAD_REQUEST,
            FactoryError::NotTheOwner | FactoryError::AgentAnswer => StatusCode::FORBIDDEN,
            FactoryError::NotFound | FactoryError::TaskNotFound => StatusCode::NOT_FOUND,
            FactoryError::RunEnded | FactoryError::AnswerRefused(_) | FactoryError::StepEnded => {
                StatusCode::CONFLICT
            }
            FactoryError::Upstream(upstream) => upstream.status_code(),
            FactoryError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for FactoryError {
    fn into_response(self) -> axum::response::Response {
        // Every error is logged once, here; the handlers' spans do not log
        // it again.
        let status_code = self.status();
        let message = match self {
            FactoryError::Upstream(upstream) => {
                tracing::warn!(error = %upstream, "the factory's workflow service failed");
                return upstream.into_response();
            }
            FactoryError::Internal(_) => {
                tracing::error!(error = ?self, "factory internal server error");
                "internal server error".to_string()
            }
            error => error.to_string(),
        };

        (
            status_code,
            Json(ErrorResponse {
                message: message.into(),
            }),
        )
            .into_response()
    }
}

/// The Run API's OpenAPI document.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_flows_handler,
        list_task_runs_handler,
        create_run_handler,
        get_run_handler,
        answer_gate_handler,
        cancel_run_handler,
        link_task_handler,
        hand_back_handler
    ),
    components(schemas(
        FlowsResponse,
        FlowSummary,
        TaskRunsResponse,
        CreateRunRequest,
        AnswerGateRequest,
        LinkTaskRequest,
        HandBackRequest,
        RunDetail,
        Run,
        RunEnd,
        RunOutcome,
        Step,
        StepEnd,
        Gate,
        GateAnswer,
        MacroUserIdStr,
        EntryId,
        Outcome,
        RunId,
        ErrorResponse,
    )),
    tags((name = "factory", description = "Runs of the software factory's Flows")),
)]
pub struct ApiDoc;
