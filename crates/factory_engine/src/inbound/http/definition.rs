//! The Run API's Factory Definition setting: the repository whose files a
//! team's Runs take their Flows, Agents and rules from
//! (`domain/definition.rs`).

use axum::{Json, extract::State, http::HeaderMap};
use macro_authorization::{MacroAuthorizationExtractor, MacroAuthorizationService, UserOrInternal};
use model_error_response::ErrorResponse;
use serde::Deserialize;

use super::{FactoryRouterState, RunErrorResponse, in_person};
use crate::domain::models::{DefinitionSettings, FactoryError};
use crate::domain::ports::FactoryService;

/// Request body for naming the team's Factory Definition.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetDefinitionRequest {
    /// The repository's `https://github.com/owner/name` URL. Null names
    /// none: the team's Runs go back to its settings and the factory's
    /// built-in Flows and Agents.
    pub repo_url: Option<String>,
}

/// The Factory Definition the caller's team names, and whether it reads at
/// its default branch's head as the caller reaches it on GitHub.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_definition",
    path = "/settings/definition",
    responses(
        (status = 200, body = DefinitionSettings),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn get_definition_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
) -> Result<Json<DefinitionSettings>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(state.service.definition(&actor).await?))
}

/// Name the repository whose files the caller's team's Runs take their
/// Flows, Agents, rules and Scorers from, or name none. The Definition must
/// read at its default branch's head as the caller reaches it (400
/// `unreachableDefinition` or `invalidDefinition`, naming the file and the
/// problem). Only an admin or the owner of the team names it, in their own
/// signed-in session (403 otherwise). While one is named, the team's
/// Control File and Standing Rules settings are refused (409
/// `definitionNamed`): the Definition's pull requests change them.
#[utoipa::path(
    put,
    tag = "factory",
    operation_id = "set_factory_definition",
    path = "/settings/definition",
    request_body = SetDefinitionRequest,
    responses(
        (status = 200, body = DefinitionSettings),
        (status = 400, body = RunErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn set_definition_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<SetDefinitionRequest>,
) -> Result<Json<DefinitionSettings>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    Ok(Json(
        state.service.set_definition(actor, body.repo_url).await?,
    ))
}
