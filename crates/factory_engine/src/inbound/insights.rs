//! Axum router for Run insights: how the caller's own Runs went over a
//! period.

#[cfg(test)]
mod test;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use chrono::NaiveDate;
use factory_records::domain::models::{RunId, RunOutcome};
use macro_authorization::{
    MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState, UserOrInternal,
};
use model_error_response::ErrorResponse;
use serde::Deserialize;

use crate::domain::insights::{
    DurationStats, EntryInsight, FlowInsight, GateInsight, InsightDay, InsightsError,
    InsightsQuery, InsightsService, NamedCount, OutcomeCounts, RunInsights, RunLink, RunTokens,
    TokenCounts, TroubleRun, UsageInsight,
};

/// Router state for Run insights.
pub struct InsightsRouterState<S, Auth> {
    service: Arc<S>,
    authorization_state: MacroAuthorizationState<Auth>,
}

impl<S, Auth> Clone for InsightsRouterState<S, Auth> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            authorization_state: self.authorization_state.clone(),
        }
    }
}

impl<S, Auth> InsightsRouterState<S, Auth>
where
    S: InsightsService,
{
    /// Create router state from a shared service and authorization state.
    pub fn new(service: Arc<S>, authorization_state: MacroAuthorizationState<Auth>) -> Self {
        Self {
            service,
            authorization_state,
        }
    }
}

impl<S, Auth> FromRef<InsightsRouterState<S, Auth>> for MacroAuthorizationState<Auth> {
    fn from_ref(state: &InsightsRouterState<S, Auth>) -> Self {
        state.authorization_state.clone()
    }
}

/// Build the insights router. Like the Run API, people call it with their
/// own login and internal callers for an acting user; either way it reads
/// only that user's Runs.
///
/// Routes:
/// - `GET /insights` — how the caller's Runs went over a period.
pub fn insights_router<S, Auth, T>(state: InsightsRouterState<S, Auth>) -> Router<T>
where
    S: InsightsService,
    Auth: MacroAuthorizationService,
    T: Send + Sync + 'static,
{
    Router::new()
        .route("/insights", get(insights_handler::<S, Auth>))
        .with_state(state)
}

/// The period, and the Flow, to read insights for.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct InsightsParams {
    /// The period's first day, `YYYY-MM-DD`.
    pub from: NaiveDate,
    /// Its last day, included. The period lasts at most 366 days.
    pub to: NaiveDate,
    /// The IANA time zone the days are in, such as `Europe/Lisbon`.
    pub time_zone: String,
    /// A built-in Flow's name, to read only its Runs.
    #[serde(default)]
    pub flow: Option<String>,
}

/// How the caller's own Runs went over a period: Runs by day, outcome and
/// Flow, how long Runs and Steps took, how often each entry failed or was
/// visited again, and the tokens their agents used. At most the newest
/// 1,000 Runs of the period are read.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_insights",
    path = "/insights",
    params(InsightsParams),
    responses(
        (status = 200, body = RunInsights),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(from = %params.from, to = %params.to))]
pub async fn insights_handler<S, Auth>(
    State(state): State<InsightsRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Query(params): Query<InsightsParams>,
) -> Result<Json<RunInsights>, InsightsError>
where
    S: InsightsService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    let query = InsightsQuery::new(params.from, params.to, &params.time_zone, params.flow)?;
    Ok(Json(state.service.insights(&actor, query).await?))
}

impl IntoResponse for InsightsError {
    fn into_response(self) -> axum::response::Response {
        let (status_code, message) = match self {
            InsightsError::Internal(_) => {
                tracing::error!(error = ?self, "factory insights failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
            error => (StatusCode::BAD_REQUEST, error.to_string()),
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

/// The insights route's OpenAPI document, merged into the Run API's.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(insights_handler),
    components(schemas(
        RunInsights,
        OutcomeCounts,
        InsightDay,
        FlowInsight,
        DurationStats,
        EntryInsight,
        GateInsight,
        NamedCount,
        TroubleRun,
        RunLink,
        RunTokens,
        TokenCounts,
        UsageInsight,
        RunId,
        RunOutcome,
        ErrorResponse,
    ))
)]
pub struct InsightsApiDoc;
