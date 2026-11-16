//! Axum router for the Run API.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, Outcome, Run, RunEnd, RunId, RunOutcome, Step, StepEnd,
};
use macro_authorization::{
    InternalOnly, MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState,
};
use macro_user_id::user_id::MacroUserIdStr;
use model_error_response::ErrorResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::gate::AnswerGate;
use crate::domain::models::{FactoryError, RunDetail};
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

/// Build the Run API router. Only internal callers may use it.
///
/// Routes:
/// - `POST /runs` — create a Run of a Flow and start it.
/// - `GET /runs/{id}` — read a Run and the Steps it took.
/// - `POST /runs/{id}/gates/{entry}/{visit}/answer` — answer the Gate the Run
///   waits on.
/// - `POST /runs/{id}/cancel` — cancel a Run that has not ended.
/// - `PUT /runs/{id}/task` — link a Run to the Macro task it works on.
pub fn factory_router<S, Auth, T>(state: FactoryRouterState<S, Auth>) -> Router<T>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
    T: Send + Sync + 'static,
{
    Router::new()
        .route("/runs", post(create_run_handler::<S, Auth>))
        .route("/runs/{id}", get(get_run_handler::<S, Auth>))
        .route(
            "/runs/{id}/gates/{entry}/{visit}/answer",
            post(answer_gate_handler::<S, Auth>),
        )
        .route("/runs/{id}/cancel", post(cancel_run_handler::<S, Auth>))
        .route("/runs/{id}/task", put(link_task_handler::<S, Auth>))
        .with_state(state)
}

/// Request body for creating a Run.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunRequest {
    /// The Flow definition the Run walks.
    #[schema(value_type = Object)]
    pub flow: serde_json::Value,
    /// What the Run is asked to do.
    pub request: String,
    /// Who starts the Run: its owner, as whom its agent Steps run.
    pub user_id: MacroUserIdStr<'static>,
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
    /// The entry whose Step asked.
    pub entry: EntryId,
    /// Which visit of the entry asked.
    pub visit: u16,
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
    /// Who answered.
    pub user_id: MacroUserIdStr<'static>,
}

/// Create a Run of a Flow and start its workflow.
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
    )
)]
#[tracing::instrument(err, skip_all)]
pub async fn create_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    _caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Json(body): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<RunDetail>), FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let created = state
        .service
        .create_run(body.flow, body.request, body.user_id, body.task_id)
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Read a Run and the Steps it took.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_run",
    path = "/runs/{id}",
    params(GetRunParams),
    responses(
        (status = 200, body = RunDetail),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(err, skip_all, fields(run_id = %params.id))]
pub async fn get_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    _caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<GetRunParams>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    Ok(Json(state.service.get_run(params.id).await?))
}

/// Answer the Gate a Run waits on. The Run goes on once its workflow took
/// the answer; reading the Run shows the answer once it is recorded.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "answer_factory_gate",
    path = "/runs/{id}/gates/{entry}/{visit}/answer",
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
    )
)]
#[tracing::instrument(err, skip_all, fields(run_id = %params.id, entry = %params.entry, visit = params.visit))]
pub async fn answer_gate_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    _caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<AnswerGateParams>,
    Json(body): Json<AnswerGateRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let answer = AnswerGate {
        entry: params.entry,
        visit: params.visit,
        answer: body.answer,
        note: body.note,
        user_id: body.user_id,
    };
    state.service.answer_gate(params.id, answer).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Cancel a Run that has not ended. Its running Step or Gate stops, and
/// reading the Run shows it `cancelled` once its workflow recorded that.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "cancel_factory_run",
    path = "/runs/{id}/cancel",
    params(GetRunParams),
    responses(
        (status = 202),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(err, skip_all, fields(run_id = %params.id))]
pub async fn cancel_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    _caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<GetRunParams>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    state.service.cancel_run(params.id).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Link a Run to the Macro task it works on, replacing any task it had.
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
#[tracing::instrument(err, skip_all, fields(run_id = %params.id))]
pub async fn link_task_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    _caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<GetRunParams>,
    Json(body): Json<LinkTaskRequest>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    Ok(Json(
        state.service.link_task(params.id, body.task_id).await?,
    ))
}

impl IntoResponse for FactoryError {
    fn into_response(self) -> axum::response::Response {
        let status_code = match &self {
            FactoryError::InvalidFlow(_)
            | FactoryError::EmptyRequest
            | FactoryError::RequestTooLarge { .. }
            | FactoryError::NoteTooLarge { .. }
            | FactoryError::UnknownTask
            | FactoryError::NotATask
            | FactoryError::UnknownUser => StatusCode::BAD_REQUEST,
            FactoryError::NotFound => StatusCode::NOT_FOUND,
            FactoryError::RunEnded | FactoryError::AnswerRefused(_) => StatusCode::CONFLICT,
            FactoryError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let message = match &self {
            FactoryError::Internal(_) => {
                tracing::error!(error=?self, "factory internal server error");
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
        create_run_handler,
        get_run_handler,
        answer_gate_handler,
        cancel_run_handler,
        link_task_handler
    ),
    components(schemas(
        CreateRunRequest,
        AnswerGateRequest,
        LinkTaskRequest,
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
