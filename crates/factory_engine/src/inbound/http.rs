//! Axum router for the Run API.

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use factory_records::domain::models::{
    EntryId, Outcome, Run, RunEnd, RunId, RunOutcome, Step, StepEnd,
};
use macro_authorization::{
    InternalOnly, MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState,
};
use model_error_response::ErrorResponse;
use serde::Deserialize;

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
pub fn factory_router<S, Auth, T>(state: FactoryRouterState<S, Auth>) -> Router<T>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
    T: Send + Sync + 'static,
{
    Router::new()
        .route("/runs", post(create_run_handler::<S, Auth>))
        .route("/runs/{id}", get(get_run_handler::<S, Auth>))
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
}

/// Path params for reading a Run.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct GetRunParams {
    /// The Run's id.
    pub id: RunId,
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
    let created = state.service.create_run(body.flow, body.request).await?;
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

impl IntoResponse for FactoryError {
    fn into_response(self) -> axum::response::Response {
        let status_code = match &self {
            FactoryError::InvalidFlow(_)
            | FactoryError::EmptyRequest
            | FactoryError::RequestTooLarge { .. } => StatusCode::BAD_REQUEST,
            FactoryError::NotFound => StatusCode::NOT_FOUND,
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
    paths(create_run_handler, get_run_handler),
    components(schemas(
        CreateRunRequest,
        RunDetail,
        Run,
        RunEnd,
        RunOutcome,
        Step,
        StepEnd,
        EntryId,
        Outcome,
        RunId,
        ErrorResponse,
    )),
    tags((name = "factory", description = "Runs of the software factory's Flows")),
)]
pub struct ApiDoc;
