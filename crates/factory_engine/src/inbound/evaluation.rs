//! Axum router for evaluation: the Scorers' judgments, the Experiments'
//! arms and the Benchmark suites of the caller's own Runs, and starting a
//! suite.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use factory_records::domain::evaluation::AgentChoices;
use factory_records::domain::models::{AgentName, Role, RunId, RunOutcome};
use macro_authorization::{
    MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState, UserOrInternal,
};
use model_error_response::ErrorResponse;
use serde::Deserialize;

use crate::domain::evaluation::insights::{
    ArmInsight, ArmScoreInsight, BenchmarkSummary, EvaluationInsights, EvaluationService,
    ExperimentInsight, ScorerInsight, SuiteError, SuiteInsight, SuiteStarted, TrialInsight,
    TrialScore,
};
use crate::domain::insights::{InsightsQuery, TokenCounts};
use crate::inbound::insights::InsightsParams;

/// Router state for evaluation.
pub struct EvaluationRouterState<S, Auth> {
    service: Arc<S>,
    authorization_state: MacroAuthorizationState<Auth>,
}

impl<S, Auth> Clone for EvaluationRouterState<S, Auth> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            authorization_state: self.authorization_state.clone(),
        }
    }
}

impl<S, Auth> EvaluationRouterState<S, Auth>
where
    S: EvaluationService,
{
    /// Create router state from a shared service and authorization state.
    pub fn new(service: Arc<S>, authorization_state: MacroAuthorizationState<Auth>) -> Self {
        Self {
            service,
            authorization_state,
        }
    }
}

impl<S, Auth> FromRef<EvaluationRouterState<S, Auth>> for MacroAuthorizationState<Auth> {
    fn from_ref(state: &EvaluationRouterState<S, Auth>) -> Self {
        state.authorization_state.clone()
    }
}

/// Build the evaluation router. As for the Run API, people call it with
/// their own login and internal callers for an acting user; either way it
/// reads, and starts suites as, only that user.
///
/// Routes:
/// - `GET /insights/evaluation` — the Scorers, Experiments and Benchmark
///   suites of the caller's Runs over a period.
/// - `POST /benchmarks/{benchmark}/suites` — start a suite.
pub fn evaluation_router<S, Auth, T>(state: EvaluationRouterState<S, Auth>) -> Router<T>
where
    S: EvaluationService,
    Auth: MacroAuthorizationService,
    T: Send + Sync + 'static,
{
    Router::new()
        .route("/insights/evaluation", get(evaluation_handler::<S, Auth>))
        .route(
            "/benchmarks/{benchmark}/suites",
            post(start_suite_handler::<S, Auth>),
        )
        .with_state(state)
}

/// How the caller's own Runs were judged and compared over a period: each
/// Scorer's judgments of a sample of them, each Experiment's arms with
/// their Runs' outcomes and scores, the Benchmarks one may replay, and the
/// suites the caller started, with each trial's score and tokens. Benchmark
/// trials are never counted with the live Runs.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_evaluation",
    path = "/insights/evaluation",
    params(InsightsParams),
    responses(
        (status = 200, body = EvaluationInsights),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(from = %params.from, to = %params.to))]
pub async fn evaluation_handler<S, Auth>(
    State(state): State<EvaluationRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Query(params): Query<InsightsParams>,
) -> Result<Json<EvaluationInsights>, crate::domain::insights::InsightsError>
where
    S: EvaluationService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    let query = InsightsQuery::new(params.from, params.to, &params.time_zone, params.flow)?;
    Ok(Json(state.service.evaluation(&actor, query).await?))
}

/// The configuration a suite's trials run on.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartSuiteRequest {
    /// The Agent of the agent definition each Role takes in place of its
    /// first, by Role; a Role left out takes the definition's.
    #[serde(default)]
    #[schema(value_type = BTreeMap<String, String>)]
    pub agents: BTreeMap<Role, AgentName>,
}

/// The Benchmark to start a suite of.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct BenchmarkPath {
    /// The Benchmark's name.
    pub benchmark: String,
}

/// Start a suite of a Benchmark: one trial Run, owned by the caller, of
/// each of its frozen tasks, on the configuration the body gives. Each trial
/// keeps a score and the tokens its agents used once its Run ends.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "start_factory_benchmark_suite",
    path = "/benchmarks/{benchmark}/suites",
    params(BenchmarkPath),
    request_body = StartSuiteRequest,
    responses(
        (status = 201, body = SuiteStarted),
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
#[tracing::instrument(skip_all, fields(benchmark = %path.benchmark))]
pub async fn start_suite_handler<S, Auth>(
    State(state): State<EvaluationRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(path): Path<BenchmarkPath>,
    Json(body): Json<StartSuiteRequest>,
) -> Result<(StatusCode, Json<SuiteStarted>), SuiteError>
where
    S: EvaluationService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    let agents: AgentChoices = body.agents;
    let started = state
        .service
        .start_suite(actor, &path.benchmark, agents)
        .await?;
    Ok((StatusCode::CREATED, Json(started)))
}

impl IntoResponse for SuiteError {
    fn into_response(self) -> axum::response::Response {
        let (status_code, message) = match self {
            SuiteError::Run(error) => return error.into_response(),
            SuiteError::Internal(_) => {
                tracing::error!(error = ?self, "starting a benchmark suite failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
            SuiteError::UnknownBenchmark(_) => (StatusCode::NOT_FOUND, self.to_string()),
            SuiteError::Configuration(_) => (StatusCode::BAD_REQUEST, self.to_string()),
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

/// The evaluation routes' OpenAPI document, merged into the Run API's.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(evaluation_handler, start_suite_handler),
    components(schemas(
        EvaluationInsights,
        ScorerInsight,
        ExperimentInsight,
        ArmInsight,
        ArmScoreInsight,
        BenchmarkSummary,
        SuiteInsight,
        TrialInsight,
        TrialScore,
        TokenCounts,
        StartSuiteRequest,
        SuiteStarted,
        RunId,
        RunOutcome,
        ErrorResponse,
    ))
)]
pub struct EvaluationApiDoc;
