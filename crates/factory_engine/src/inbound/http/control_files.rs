//! The Run API's Control File settings: the patterns a team's Runs are
//! checked against (`domain/control_files.rs`).

use axum::{Json, extract::State, http::HeaderMap};
use macro_authorization::{MacroAuthorizationExtractor, MacroAuthorizationService, UserOrInternal};
use model_error_response::ErrorResponse;
use serde::Deserialize;

use super::{FactoryRouterState, in_person};
use crate::domain::models::{ControlFilesSettings, FactoryError};
use crate::domain::ports::FactoryService;

/// Request body for setting the Control File patterns.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetControlFilesRequest {
    /// The glob patterns, matched against each changed path from the
    /// repository's root: `*` stays within one folder, `**` spans any
    /// number. At most 200, each at most 256 bytes. Null goes back to the
    /// factory's default; an empty list checks no file.
    pub patterns: Option<Vec<String>>,
}

/// The Control File patterns the caller's Runs are checked against: their
/// team's, or the factory's default, and whether the caller may change
/// them.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_control_files",
    path = "/settings/control-files",
    responses(
        (status = 200, body = ControlFilesSettings),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn get_control_files_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
) -> Result<Json<ControlFilesSettings>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(state.service.control_files(&actor).await?))
}

/// Set the Control File patterns the Runs of the caller's team are checked
/// against, or go back to the factory's default. Only an admin or the owner
/// of the team sets them, and a shorter list lets a Run's agent change more
/// files unseen, so only in their own signed-in session (403 otherwise).
#[utoipa::path(
    put,
    tag = "factory",
    operation_id = "set_factory_control_files",
    path = "/settings/control-files",
    request_body = SetControlFilesRequest,
    responses(
        (status = 200, body = ControlFilesSettings),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn set_control_files_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<SetControlFilesRequest>,
) -> Result<Json<ControlFilesSettings>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    Ok(Json(
        state
            .service
            .set_control_files(actor, body.patterns)
            .await?,
    ))
}
