//! `GET /runs`: the factory's board, a page at a time; and
//! `POST /runs/by-pull-requests`: the Runs that opened some pull requests.

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use factory_records::domain::board::BoardState;
use factory_records::domain::models::RunId;
use macro_authorization::{MacroAuthorizationExtractor, MacroAuthorizationService, UserOrInternal};
use model_error_response::ErrorResponse;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::FactoryRouterState;
use crate::domain::board::{BoardFilter, BoardNext, BoardPage, BoardScope, PullRequestRuns};
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

/// Which Runs a page of the board lists. Every filter left out lists them
/// all.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query, rename_all = "camelCase")]
pub struct BoardParams {
    /// Whose Runs: the caller's (`mine`, when left out), or their team's
    /// (`team`), which lists the members' Runs on tasks the caller can see.
    #[serde(default)]
    pub scope: Option<BoardScope>,
    /// Only Runs there.
    #[serde(default)]
    pub state: Option<BoardState>,
    /// Only Runs of this built-in Flow, by name.
    #[serde(default)]
    pub flow: Option<String>,
    /// Only Runs on this repository, by its `https://github.com/owner/name`
    /// URL.
    #[serde(default)]
    pub repo_url: Option<String>,
    /// Only Runs whose task was in this Project (an initiative) when they
    /// started.
    #[serde(default)]
    pub project_id: Option<Uuid>,
    /// Only Runs whose request, or whose task's name, has these words in
    /// it, in any case: at most 200 characters are read.
    #[serde(default)]
    pub search: Option<String>,
    /// Where the page starts: the `next.createdAt` of the page before.
    #[serde(default)]
    pub after_created_at: Option<DateTime<Utc>>,
    /// The `next.id` of the page before: given with `afterCreatedAt` or
    /// not at all (400 otherwise).
    #[serde(default)]
    pub after_id: Option<RunId>,
    /// The most Runs the page lists: 25 when left out, at most 50.
    #[serde(default)]
    pub limit: Option<u16>,
}

/// List a page of the factory's board: the Runs the caller can see,
/// theirs or their team's, newest first, each with its Steps and where it
/// is (`running`, `waiting` on its owner at a Gate, `held` by a Main Hold,
/// `stopped` for a person, or `finished`). Read the next page with `next`.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "list_factory_board",
    path = "/runs",
    params(BoardParams),
    responses(
        (status = 200, body = BoardPage),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn list_board_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Query(params): Query<BoardParams>,
) -> Result<Json<BoardPage>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    let after = match (params.after_created_at, params.after_id) {
        (Some(created_at), Some(id)) => Some(BoardNext { created_at, id }),
        (None, None) => None,
        _ => return Err(FactoryError::HalfCursor),
    };
    let filter = BoardFilter {
        scope: params.scope.unwrap_or_default(),
        state: params.state,
        flow: params.flow,
        repo_url: params.repo_url,
        project_id: params.project_id,
        search: BoardFilter::search_words(params.search),
        after,
        limit: BoardFilter::page_size(params.limit),
    };
    Ok(Json(state.service.board(&actor, filter).await?))
}

/// GitHub pull requests, by their pages.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestUrls {
    /// The pull requests' pages, such as
    /// `https://github.com/owner/repo/pull/12`: at most 100.
    pub urls: Vec<String>,
}

/// Find the Runs that opened some pull requests: for each one, the newest
/// Run the caller can see on their team's board (theirs, or a team
/// member's on a task they can see), with its Steps, where it is, and the
/// merge Gate it waits on. A pull request no such Run opened is left out,
/// whether no Run opened it or one the caller cannot see did.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "find_pull_request_runs",
    path = "/runs/by-pull-requests",
    request_body = PullRequestUrls,
    responses(
        (status = 200, body = PullRequestRuns),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn pull_request_runs_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Json(body): Json<PullRequestUrls>,
) -> Result<Json<PullRequestRuns>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(
        state.service.pull_request_runs(&actor, body.urls).await?,
    ))
}
