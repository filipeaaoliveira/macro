//! The Run API's CI quarantines by hand: a person quarantines a CI job of a
//! repository, so the `rerun_flaky_ci` rule takes its failures for flaky,
//! or releases it (`domain/service/quarantine.rs`).

use axum::{Json, extract::State, http::HeaderMap};
use macro_authorization::{MacroAuthorizationExtractor, MacroAuthorizationService, UserOrInternal};
use model_error_response::ErrorResponse;
use serde::Deserialize;

use super::{FactoryRouterState, in_person};
use crate::domain::models::{CiQuarantineChange, FactoryError};
use crate::domain::ports::FactoryService;

/// The CI job to quarantine or release.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CiJobRequest {
    /// The repository's `https://github.com/owner/name` URL.
    pub repository: String,
    /// The job's name: its check run's, or its commit status's context.
    pub job: String,
}

/// Quarantine a CI job of a repository: every failure of it is a known
/// flake there, which the `rerun_flaky_ci` Standing Rule reruns once before
/// any CI fix. Quarantines are per repository, so this reaches every team's
/// Runs on it, not only the caller's team's. It files no task; each passed
/// rerun of the job extends it, and it lapses 30 days after it was made or
/// a rerun of it last passed. Quarantining a job that is so already
/// changes nothing. Only an owner or admin of a team, from an account that
/// is not an agent's, in their own signed-in session (403 otherwise), who
/// may push to the repository (400 otherwise). The job's name is trimmed,
/// and cut as CI health keeps job names.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "quarantine_factory_ci_job",
    path = "/insights/ci-quarantines",
    request_body = CiJobRequest,
    responses(
        (status = 200, body = CiQuarantineChange),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn quarantine_ci_job_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<CiJobRequest>,
) -> Result<Json<CiQuarantineChange>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    Ok(Json(
        state
            .service
            .quarantine_ci_job(&actor, &body.repository, &body.job)
            .await?,
    ))
}

/// Release a CI job of a repository: lift every quarantine of it, made by
/// hand or by its passed reruns, for every team's Runs on it. Its reruns and
/// greens before the release no longer prove it flaky, so its failures go
/// to a CI fix again until new evidence, or a person, quarantines it anew;
/// a job the Factory Definition names in `knownFlakes` stays a known flake.
/// Releasing a job that is not quarantined changes nothing. The same
/// people as for a quarantine may.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "release_factory_ci_job",
    path = "/insights/ci-quarantines/release",
    request_body = CiJobRequest,
    responses(
        (status = 200, body = CiQuarantineChange),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn release_ci_job_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<CiJobRequest>,
) -> Result<Json<CiQuarantineChange>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    Ok(Json(
        state
            .service
            .release_ci_job(&actor, &body.repository, &body.job)
            .await?,
    ))
}
