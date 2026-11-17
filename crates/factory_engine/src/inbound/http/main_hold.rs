//! `POST /internal/main-holds/clear`: an operator clears a Main Hold no look
//! can clear. Internal callers only, and left out of the Run API's OpenAPI
//! document, so no client or SDK offers it.

use axum::Json;
use axum::extract::State;
use macro_authorization::{InternalOnly, MacroAuthorizationExtractor, MacroAuthorizationService};
use serde::{Deserialize, Serialize};

use super::FactoryRouterState;
use crate::domain::models::FactoryError;
use crate::domain::ports::FactoryService;

/// The base branch whose hold to clear.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearMainHoldRequest {
    /// The repository's `https://github.com/owner/name` URL.
    pub repo_url: String,
    /// The branch Runs merge to.
    pub base_branch: String,
}

/// Whether the base was held.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearMainHoldResponse {
    /// Whether a hold was set and is now cleared.
    pub held: bool,
}

/// Clear the hold on a base branch. Its watch, if one runs, holds it again
/// on its next red look.
pub async fn clear_main_hold_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    _internal: MacroAuthorizationExtractor<Auth, InternalOnly>,
    Json(body): Json<ClearMainHoldRequest>,
) -> Result<Json<ClearMainHoldResponse>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let held = state
        .service
        .clear_main_hold(&body.repo_url, &body.base_branch)
        .await?;
    Ok(Json(ClearMainHoldResponse { held }))
}
