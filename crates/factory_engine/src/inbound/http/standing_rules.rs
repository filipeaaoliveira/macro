//! The Run API's Standing Rules settings: which kinds of Gate the factory
//! answers on its own for a team's Runs (`domain/standing_rules.rs`), and
//! for the Runs whose task is in one of its Projects.

use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
};
use factory_records::domain::standing_rules::OrchestratorDefinition;
use macro_authorization::{MacroAuthorizationExtractor, MacroAuthorizationService, UserOrInternal};
use model_error_response::ErrorResponse;
use serde::Deserialize;
use uuid::Uuid;

use super::{FactoryRouterState, in_person};
use crate::domain::models::{FactoryError, StandingRulesSettings};
use crate::domain::ports::FactoryService;

/// Request body for setting an Orchestrator Definition.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetStandingRulesRequest {
    /// Which kinds of Gate the factory answers on its own. Null goes back
    /// to the factory's default (a team's), or removes the Project's own
    /// (a Project's).
    pub definition: Option<OrchestratorDefinition>,
}

/// Path params naming a Project.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct ProjectRulesParams {
    /// The Project: an initiative's id.
    pub project_id: Uuid,
}

/// The Standing Rules the caller's Runs are created with: their team's, or
/// the factory's default, the team's Projects' own, and whether the caller
/// may change them.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_standing_rules",
    path = "/settings/standing-rules",
    responses(
        (status = 200, body = StandingRulesSettings),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn get_standing_rules_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
) -> Result<Json<StandingRulesSettings>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(state.service.standing_rules(&actor).await?))
}

/// Set the Orchestrator Definition of the caller's team, or go back to the
/// factory's default. It applies to Runs created from now on. Only an admin
/// or the owner of the team sets it, in their own signed-in session (403
/// otherwise): a rule answers Gates in its place.
#[utoipa::path(
    put,
    tag = "factory",
    operation_id = "set_factory_standing_rules",
    path = "/settings/standing-rules",
    request_body = SetStandingRulesRequest,
    responses(
        (status = 200, body = StandingRulesSettings),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn set_standing_rules_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<SetStandingRulesRequest>,
) -> Result<Json<StandingRulesSettings>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    Ok(Json(
        state
            .service
            .set_standing_rules(actor, body.definition)
            .await?,
    ))
}

/// Set the Orchestrator Definition of the caller's team for one Project,
/// which replaces the team's for Runs created on a task in it, or remove
/// it. The caller must administer the team, in their own signed-in session
/// (403 otherwise), and be able to edit the Project (400 otherwise, as for
/// one that does not exist).
#[utoipa::path(
    put,
    tag = "factory",
    operation_id = "set_factory_project_standing_rules",
    path = "/settings/standing-rules/projects/{project_id}",
    params(ProjectRulesParams),
    request_body = SetStandingRulesRequest,
    responses(
        (status = 200, body = StandingRulesSettings),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(project_id = %params.project_id))]
pub async fn set_project_standing_rules_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    Path(params): Path<ProjectRulesParams>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<SetStandingRulesRequest>,
) -> Result<Json<StandingRulesSettings>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    Ok(Json(
        state
            .service
            .set_project_rules(actor, params.project_id, body.definition)
            .await?,
    ))
}
