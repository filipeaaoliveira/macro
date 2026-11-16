//! Axum router for the Run API.

#[cfg(test)]
mod test;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, HandBack, Outcome, Run, RunEnd, RunId, RunOutcome, Step, StepEnd,
};
use macro_authorization::{
    InternalAuthorization, InternalOnly, MacroAuthorizationExtractor, MacroAuthorizationService,
    MacroAuthorizationState, UserOnly,
};
use macro_user_id::user_id::MacroUserIdStr;
use model_error_response::ErrorResponse;
use serde::Deserialize;
use uuid::Uuid;

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

/// Build the Run API router. Internal callers use it for an acting user (the
/// identity headers of the internal key), and reach only that user's Runs.
/// `POST /hand-backs` is the exception: an agent session's code-mode SDK
/// calls it as the Run's owner.
///
/// Routes:
/// - `POST /runs` — create a Run of a Flow, owned by the acting user, and
///   start it.
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
        .route("/runs", post(create_run_handler::<S, Auth>))
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

/// The user an internal caller acts for. Every Run call but a hand back
/// needs one. Its id is lowercase, whatever case the caller named it in: a
/// `MacroUserIdStr` lowercases what it parses, as `"User".id` is stored.
fn acting_user(caller: InternalAuthorization) -> Result<MacroUserIdStr<'static>, FactoryError> {
    caller
        .acting_user
        .map(|user| user.macro_user_id)
        .ok_or(FactoryError::NoActingUser)
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

/// Create a Run of a Flow, owned by the acting user, and start its workflow.
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
    caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Json(body): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<RunDetail>), FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = acting_user(caller.authorization)?;
    let created = state
        .service
        .create_run(actor, body.flow, body.request, body.task_id)
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Read one of the acting user's Runs and the Steps it took.
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
    caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<GetRunParams>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = acting_user(caller.authorization)?;
    Ok(Json(state.service.get_run(&actor, params.id).await?))
}

/// Answer, as the acting user, the Gate one of their Runs waits on. The Run
/// goes on once its workflow took the answer; reading the Run shows the
/// answer once it is recorded.
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
    caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<AnswerGateParams>,
    Json(body): Json<AnswerGateRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = acting_user(caller.authorization)?;
    state
        .service
        .answer_gate(actor, params.id, params.seq, body.answer, body.note)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// Cancel one of the acting user's Runs. Its running Step or Gate stops, and
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
    caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<GetRunParams>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = acting_user(caller.authorization)?;
    state.service.cancel_run(&actor, params.id).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Link one of the acting user's Runs to a Macro task they can see,
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
    caller: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Path(params): Path<GetRunParams>,
    Json(body): Json<LinkTaskRequest>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = acting_user(caller.authorization)?;
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
            | FactoryError::NoActingUser
            | FactoryError::AnswerNotOffered { .. }
            | FactoryError::NotAStepSession
            | FactoryError::OutcomeNotOffered { .. }
            | FactoryError::HandBackTooLarge { .. } => StatusCode::BAD_REQUEST,
            FactoryError::NotTheOwner => StatusCode::FORBIDDEN,
            FactoryError::NotFound => StatusCode::NOT_FOUND,
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
        create_run_handler,
        get_run_handler,
        answer_gate_handler,
        cancel_run_handler,
        link_task_handler,
        hand_back_handler
    ),
    components(schemas(
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
