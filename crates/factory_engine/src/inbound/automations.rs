//! Axum router for Automations: a team's standing orders to start Runs,
//! and the calls that fire them.

#[cfg(test)]
mod test;

use std::sync::Arc;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{FromRef, Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post, put},
};
use factory_records::domain::automations::{
    Automation, AutomationTrigger, FiringSkip, TriggerKind,
};
use macro_authorization::{
    MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState, UserOnly,
    UserOrInternal,
};
use model_error_response::ErrorResponse;
use serde::Deserialize;
use uuid::Uuid;

use crate::domain::automations::{
    AutomationRequest, AutomationsSettings, CreatedAutomation, FiringResult, GithubFiring,
    RoutineFiring, WebhookFiring,
};
use crate::domain::models::FactoryError;
use crate::domain::ports::AutomationService;
use crate::inbound::http::{RunErrorCode, RunErrorResponse, is_in_person, program_agent};

/// Router state for Automations.
pub struct AutomationsRouterState<S, Auth> {
    service: Arc<S>,
    authorization_state: MacroAuthorizationState<Auth>,
}

impl<S, Auth> Clone for AutomationsRouterState<S, Auth> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            authorization_state: self.authorization_state.clone(),
        }
    }
}

impl<S, Auth> AutomationsRouterState<S, Auth>
where
    S: AutomationService,
{
    /// Create router state from a shared service and authorization state.
    pub fn new(service: Arc<S>, authorization_state: MacroAuthorizationState<Auth>) -> Self {
        Self {
            service,
            authorization_state,
        }
    }
}

impl<S, Auth> FromRef<AutomationsRouterState<S, Auth>> for MacroAuthorizationState<Auth> {
    fn from_ref(state: &AutomationsRouterState<S, Auth>) -> Self {
        state.authorization_state.clone()
    }
}

/// Build the Automations router. A team's members read its Automations;
/// each makes their own, in person, which act as them; a webhook's calls
/// carry no login, only the signature of its secret.
///
/// Routes:
/// - `GET /automations` — the caller's team's Automations.
/// - `POST /automations` — make one, acting as the caller.
/// - `PUT /automations/{id}/enabled` — switch one on or off.
/// - `DELETE /automations/{id}` — delete one.
/// - `POST /automations/fire` — an Automation's routine fires it, from
///   its code step.
/// - `POST /automations/{id}/webhook` — a signed call fires a webhook
///   Automation.
pub fn automations_router<S, Auth, T>(state: AutomationsRouterState<S, Auth>) -> Router<T>
where
    S: AutomationService,
    Auth: MacroAuthorizationService,
    T: Send + Sync + 'static,
{
    Router::new()
        .route(
            "/automations",
            get(list_automations_handler::<S, Auth>).post(create_automation_handler::<S, Auth>),
        )
        .route(
            "/automations/{id}/enabled",
            put(set_automation_enabled_handler::<S, Auth>),
        )
        .route(
            "/automations/{id}",
            delete(delete_automation_handler::<S, Auth>),
        )
        .route(
            "/automations/fire",
            post(fire_automation_handler::<S, Auth>),
        )
        .route(
            "/automations/{id}/webhook",
            post(webhook_handler::<S, Auth>),
        )
        .with_state(state)
}

/// Path params naming an Automation.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct AutomationParams {
    /// The Automation's id.
    pub id: Uuid,
}

/// Request body for switching an Automation on or off.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SetAutomationEnabledRequest {
    /// Whether it starts Runs.
    pub enabled: bool,
}

/// The caller's team's Automations, newest first, and whether the caller
/// administers the team. A person on no team has none.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "list_factory_automations",
    path = "/automations",
    responses(
        (status = 200, body = AutomationsSettings),
        (status = 401, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn list_automations_handler<S, Auth>(
    State(state): State<AutomationsRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
) -> Result<Json<AutomationsSettings>, FactoryError>
where
    S: AutomationService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(state.service.automations(&actor).await?))
}

/// Make an Automation of the caller's team, which starts Runs as the
/// caller, with their permissions, each time its trigger fires. Only the
/// caller in their own signed-in session makes one (403 otherwise), and
/// its Runs must be ones they could start now. A schedule, a repository
/// event and a Task event are a routine of the caller's, which the
/// scheduler must take (400 with its reason otherwise); a webhook's secret
/// is in the answer, once.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "create_factory_automation",
    path = "/automations",
    request_body = AutomationRequest,
    responses(
        (status = 201, body = CreatedAutomation),
        (status = 400, body = RunErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn create_automation_handler<S, Auth>(
    State(state): State<AutomationsRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<AutomationRequest>,
) -> Result<(StatusCode, Json<CreatedAutomation>), FactoryError>
where
    S: AutomationService,
    Auth: MacroAuthorizationService,
{
    let in_person = is_in_person(&caller.authorization, &headers);
    let actor = caller.authorization.user.macro_user_id;
    let created = state
        .service
        .create_automation(actor, in_person, body)
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// Switch an Automation of the caller's team on or off. Only its creator
/// switches it on, in their own signed-in session, as for making it; its
/// creator or an admin of the team switches it off (403 otherwise). Its
/// routine follows.
#[utoipa::path(
    put,
    tag = "factory",
    operation_id = "set_factory_automation_enabled",
    path = "/automations/{id}/enabled",
    params(AutomationParams),
    request_body = SetAutomationEnabledRequest,
    responses(
        (status = 200, body = Automation),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(automation_id = %params.id))]
pub async fn set_automation_enabled_handler<S, Auth>(
    State(state): State<AutomationsRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AutomationParams>,
    Json(body): Json<SetAutomationEnabledRequest>,
) -> Result<Json<Automation>, FactoryError>
where
    S: AutomationService,
    Auth: MacroAuthorizationService,
{
    let in_person = is_in_person(&caller.authorization, &headers);
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(
        state
            .service
            .set_automation_enabled(&actor, in_person, params.id, body.enabled)
            .await?,
    ))
}

/// Delete an Automation of the caller's team, its creator's or, for an
/// admin of the team, any, and its routine. The Runs it started keep the record of it.
#[utoipa::path(
    delete,
    tag = "factory",
    operation_id = "delete_factory_automation",
    path = "/automations/{id}",
    params(AutomationParams),
    responses(
        (status = 204),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(automation_id = %params.id))]
pub async fn delete_automation_handler<S, Auth>(
    State(state): State<AutomationsRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<AutomationParams>,
) -> Result<StatusCode, FactoryError>
where
    S: AutomationService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    state.service.delete_automation(&actor, params.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Fire the Automation whose routine this run is: only the code step of a
/// run of an Automation's routine calls it, with the credential the
/// scheduler signed for the routine (403 for any other caller). It starts
/// a Run as the Automation's creator, once per routine run, unless the
/// Automation is switched off, the queue is full or a Run already runs on
/// the task. A routine run for an agent, or in an agent session, starts it
/// as that agent: under a Definition with Standing Roles its bot must fill
/// one that allows `start`, or the firing is skipped as `refused`.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "fire_factory_automation",
    path = "/automations/fire",
    request_body = RoutineFiring,
    responses(
        (status = 200, body = FiringResult),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn fire_automation_handler<S, Auth>(
    State(state): State<AutomationsRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOnly>,
    Json(body): Json<RoutineFiring>,
) -> Result<Json<FiringResult>, FactoryError>
where
    S: AutomationService,
    Auth: MacroAuthorizationService,
{
    // The routine the credential is signed for, as the correlation layer
    // read it; no other caller names one.
    let routine_id =
        agent_correlation::context::current_routine().ok_or(FactoryError::NotARoutine)?;
    // A routine the scheduler runs as its creator acts as them; one run for
    // an agent, or in an agent session, acts as that agent, which Standing
    // Roles check as for `POST /runs`. The Automation's own routine is an
    // `Agent` routine signed for the Macro routine bot: the service, which
    // knows the routine is the Automation's, reads that as its creator.
    let acting = program_agent(&caller.authorization);
    Ok(Json(
        state
            .service
            .fire_from_routine(
                &caller.authorization.macro_user_id,
                routine_id,
                body,
                acting,
            )
            .await?,
    ))
}

/// Fire a webhook Automation. The call carries no login: it is signed with
/// the Automation's secret, as Macro signs its own webhooks
/// (`x-macro-signature: v1=<hex>`, an HMAC-SHA256 of
/// `"{x-macro-timestamp}.{body}"`), at a timestamp (Unix seconds) within
/// five minutes of now (401 otherwise, as for an id no webhook has). The
/// same timestamp and body received before is refused (409), however its
/// digest is spelled, so a replay starts nothing. The body is empty, or JSON naming the
/// task the Run works on.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "call_factory_automation_webhook",
    path = "/automations/{id}/webhook",
    params(AutomationParams),
    request_body(content = Option<WebhookFiring>, content_type = "application/json"),
    responses(
        (status = 202, body = FiringResult),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(automation_id = %params.id))]
pub async fn webhook_handler<S, Auth>(
    State(state): State<AutomationsRouterState<S, Auth>>,
    Path(params): Path<AutomationParams>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<FiringResult>), FactoryError>
where
    S: AutomationService,
    Auth: MacroAuthorizationService,
{
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .ok_or(FactoryError::BadSignature)
    };
    let timestamp = header(webhook_signature::TIMESTAMP_HEADER)?;
    let signature = header(webhook_signature::SIGNATURE_HEADER)?;
    let fired = state
        .service
        .fire_webhook(params.id, timestamp, signature, &body)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(fired)))
}

/// The Automations API's OpenAPI document.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_automations_handler,
        create_automation_handler,
        set_automation_enabled_handler,
        delete_automation_handler,
        fire_automation_handler,
        webhook_handler,
    ),
    components(schemas(
        AutomationsSettings,
        Automation,
        AutomationTrigger,
        TriggerKind,
        AutomationRequest,
        CreatedAutomation,
        SetAutomationEnabledRequest,
        RoutineFiring,
        GithubFiring,
        WebhookFiring,
        FiringResult,
        FiringSkip,
        RunErrorResponse,
        RunErrorCode,
        ErrorResponse,
    )),
    tags((name = "factory", description = "Runs of the software factory's Flows")),
)]
pub struct AutomationsApiDoc;
