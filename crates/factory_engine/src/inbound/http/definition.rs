//! The Run API's Factory Definition setting: the repository whose files a
//! team's Runs take their Flows, Agents and rules from
//! (`domain/definition.rs`), the text of one of its files, and changes to
//! it checked or proposed as pull requests.

use std::collections::BTreeMap;

use axum::{
    Json,
    extract::{Query, State},
    http::HeaderMap,
};
use macro_authorization::{MacroAuthorizationExtractor, MacroAuthorizationService, UserOrInternal};
use model_error_response::ErrorResponse;
use serde::Deserialize;
use uuid::Uuid;

use super::{FactoryRouterState, RunErrorResponse, in_person};
use crate::domain::definition::ProposedChange;
use crate::domain::models::{
    DefinitionChangeCheck, DefinitionFile, DefinitionPullRequest, DefinitionSettings, FactoryError,
};
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

/// Request body for proposing a change to the team's Factory Definition.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProposeDefinitionChangeRequest {
    /// The change's id, a UUID the client makes once per change: sending
    /// the change again with it, as after a lost answer, answers the pull
    /// request it opened, open or merged, without checking or writing
    /// anything again (409 `definitionChangeClosed` once it was closed
    /// unmerged).
    pub change_id: Uuid,
    /// What the change does, in a line: its pull request's title.
    pub title: String,
    /// Why, for whoever reviews the pull request.
    #[serde(default)]
    pub description: Option<String>,
    /// The files it writes, each by its path from the repository's root
    /// (`factory.json`, `flows/{name}.json`, `agents.json`, `rules.json`,
    /// `scorers.json`), with its whole new text. Files left out stay as
    /// they are.
    pub files: BTreeMap<String, String>,
}

/// Propose a change to the caller's team's Factory Definition as a pull
/// request on its repository, and answer that pull request. The Definition
/// must read with the change at its default branch's head, as a Run's start
/// reads it (400 `invalidDefinitionChange`, naming the file and the
/// problem); each file written must be one it then reads. The change is
/// committed on its own new branch, `definition-change/{changeId}`, never
/// on the default branch, with the caller's GitHub account, which must
/// push to the repository (400 `noPushAccess`); a person merges it. A
/// branch GitHub will not create (a rule of the repository, or one of that
/// name already there with no pull request) or a pull request it will not
/// open is 409 `definitionChangeNotOpened`. Only an admin of the team
/// proposes, in their own signed-in session (403 otherwise, before the
/// change is checked); a team that names no Definition has none to change
/// (409 `noDefinition`).
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "propose_factory_definition_change",
    path = "/settings/definition/changes",
    request_body = ProposeDefinitionChangeRequest,
    responses(
        (status = 200, body = DefinitionPullRequest),
        (status = 400, body = RunErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = RunErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn propose_definition_change_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<ProposeDefinitionChangeRequest>,
) -> Result<Json<DefinitionPullRequest>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    let change = ProposedChange {
        title: body.title,
        description: body.description,
        files: body.files,
    };
    Ok(Json(
        state
            .service
            .propose_definition_change(actor, body.change_id, change)
            .await?,
    ))
}

/// Which file of the team's Factory Definition to read.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DefinitionFileParams {
    /// Its path from the repository's root: `factory.json`,
    /// `flows/{name}.json`, `agents.json`, `rules.json` or `scorers.json`.
    pub path: String,
}

/// The whole text of one file of the caller's team's Factory Definition, as
/// it is at its default branch's head, the commit a change proposed now
/// goes on top of, read as the caller reaches it on GitHub. A file not
/// there yet, as a Flow being added, answers no text. A path that names no
/// Definition file is 400; a repository the caller does not reach is 400
/// `unreachableDefinition`; a file GitHub will not give (too large) is 400
/// `invalidDefinition`; a team that names no Definition is 409
/// `noDefinition`.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_definition_file",
    path = "/settings/definition/file",
    params(DefinitionFileParams),
    responses(
        (status = 200, body = DefinitionFile),
        (status = 400, body = RunErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 409, body = RunErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn get_definition_file_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Query(params): Query<DefinitionFileParams>,
) -> Result<Json<DefinitionFile>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(
        state.service.definition_file(&actor, params.path).await?,
    ))
}

/// Request body for checking a change to the team's Factory Definition
/// without proposing it.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckDefinitionChangeRequest {
    /// What the change does, in a line.
    pub title: String,
    /// Why, for whoever reviews the pull request.
    #[serde(default)]
    pub description: Option<String>,
    /// The files it writes, by path, each with its whole new text, as
    /// `POST /settings/definition/changes` takes them.
    pub files: BTreeMap<String, String>,
}

/// Check a change to the caller's team's Factory Definition as `POST
/// /settings/definition/changes` would, without proposing it: nothing is
/// written to GitHub. It is refused with the same statuses and codes (403
/// for someone who does not administer the team, in person; 409
/// `noDefinition`; 400 `noPushAccess`, `unreachableDefinition`,
/// `invalidDefinition` or `invalidDefinitionChange`), and answers the
/// commit it was checked against. A pull request sent later is checked
/// again, against the head it is sent at.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "check_factory_definition_change",
    path = "/settings/definition/changes/check",
    request_body = CheckDefinitionChangeRequest,
    responses(
        (status = 200, body = DefinitionChangeCheck),
        (status = 400, body = RunErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = RunErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn check_definition_change_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<CheckDefinitionChangeRequest>,
) -> Result<Json<DefinitionChangeCheck>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    let change = ProposedChange {
        title: body.title,
        description: body.description,
        files: body.files,
    };
    Ok(Json(
        state.service.check_definition_change(actor, change).await?,
    ))
}
