//! Axum router for the Run API.

mod board;
mod ci_quarantine;
mod control_files;
mod definition;
mod main_hold;
mod standing_rules;
#[cfg(test)]
mod test;

use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{FromRef, Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post, put},
};
use factory_records::domain::board::BoardState;
use factory_records::domain::models::{
    AskedBack, ControlFileChanges, DefinitionRead, EntryId, FlawMarker, Gate, GateAnswer,
    GateModelReading, HandBack, Outcome, Run, RunDefinition, RunEnd, RunFlaw, RunId, RunOutcome,
    RunRetry, StandInAnswer, Steer, SteerAction, Step, StepEnd, StepRuntime, StepTerminal,
};
use macro_authorization::{
    MacroAuthorizationExtractor, MacroAuthorizationService, MacroAuthorizationState, UserOnly,
    UserOrInternal, UserOrInternalAuthorization, UserOrInternalCaller, is_program_run,
};
use macro_user_id::user_id::MacroUserIdStr;
use model_error_response::ErrorResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::board::{
    BoardNext, BoardPage, BoardRun, BoardScope, PullRequestRun, PullRequestRuns,
};
use crate::domain::models::{
    ActingAgent, Answerer, CiQuarantineChange, ControlFilesSettings, DefinitionChangeCheck,
    DefinitionContents, DefinitionFile, DefinitionFlow, DefinitionPullRequest, DefinitionRole,
    DefinitionScorer, DefinitionSettings, FactoryError, FlowSummary, PersonOnly, RunDetail,
    SessionStepRef, StandingRulesSettings,
};
use crate::domain::ports::FactoryService;
use crate::domain::step_terminal::TerminalRequest;
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, OrchestratorKinds, ProjectRules, RulesSource, RunRules, StandInKinds,
    StandingRule,
};

/// Router state for the Run API.
pub struct FactoryRouterState<S, Auth> {
    service: Arc<S>,
    authorization_state: MacroAuthorizationState<Auth>,
}

impl<S, Auth> Clone for FactoryRouterState<S, Auth> {
    fn clone(&self) -> Self {
        Self {
            service: self.service.clone(),
            authorization_state: self.authorization_state.clone(),
        }
    }
}

impl<S, Auth> FactoryRouterState<S, Auth>
where
    S: FactoryService,
{
    /// Create router state from a shared service and authorization state.
    pub fn new(service: Arc<S>, authorization_state: MacroAuthorizationState<Auth>) -> Self {
        Self {
            service,
            authorization_state,
        }
    }
}

impl<S, Auth> FromRef<FactoryRouterState<S, Auth>> for MacroAuthorizationState<Auth> {
    fn from_ref(state: &FactoryRouterState<S, Auth>) -> Self {
        state.authorization_state.clone()
    }
}

/// Build the Run API router. People call it with their own login, and
/// internal callers for an acting user (the identity headers of the internal
/// key); either way the call reaches only that user's Runs. `POST
/// /hand-backs` is an agent session's: its code-mode SDK calls it as the
/// Run's owner.
///
/// Routes:
/// - `GET /flows` — the built-in Flows a Run can be started of.
/// - `GET /runs` — a page of the board: the caller's Runs, or their
///   team's on tasks they can see, newest first, by where each one is.
/// - `POST /runs/by-pull-requests` — the Runs that opened some pull
///   requests, among those the caller's team board shows.
/// - `POST /runs` — create a Run of a built-in Flow, owned by the acting
///   user, and start it, unless it holds as many places in its Queue as the
///   Queue limit: Runs waiting on it, and Runs yet to reach their first Gate.
/// - `GET /tasks/{task_id}/runs` — the caller's Runs on a task, newest
///   first.
/// - `GET /runs/waiting` — the caller's Runs that wait on them at a Gate,
///   newest first, and those that hold a place before their first Gate.
/// - `GET /runs/{id}` — read a Run and the Steps it took: the caller's, or
///   a team member's on a task they can see.
/// - `POST /runs/{id}/steps/{seq}/answer` — answer the Gate of the Run's
///   Step `seq`.
/// - `POST /runs/{id}/steps/{seq}/reply` — reply to it in words, which the
///   factory reads as one of its answers or asks back.
/// - `GET /sessions/{session_id}/step` — the caller's factory Step that
///   works in an agent session, so the session's view sends them to its
///   Run rather than let them prompt it.
/// - `POST /runs/{id}/steps/{seq}/note` — send the agent of the Run's
///   running Step `seq` a note, from its Step Terminal.
/// - `POST /runs/{id}/steps/{seq}/pause` — pause it.
/// - `POST /runs/{id}/steps/{seq}/resume` — resume it, with a note or
///   without.
/// - `POST /runs/{id}/steps/{seq}/stop` — stop it: it ends `failed`.
/// - `POST /runs/{id}/cancel` — cancel a Run.
/// - `POST /runs/{id}/flaw` — mark a Run that ended `success` flawed, with
///   why.
/// - `PUT /runs/{id}/task` — link a Run to the Macro task it works on.
/// - `POST /hand-backs` — an agent Step's session hands back its result.
/// - `GET /settings/control-files` — the Control File patterns the caller's
///   Runs are checked against.
/// - `PUT /settings/control-files` — set them, or go back to the default.
/// - `GET /settings/definition` — the Factory Definition the caller's team
///   names, and whether it reads.
/// - `PUT /settings/definition` — name one, or none.
/// - `GET /settings/definition/file` — one of its files' text at its head.
/// - `POST /settings/definition/changes` — propose a change to it as a
///   pull request.
/// - `POST /settings/definition/changes/check` — check a change as
///   proposing it would, writing nothing.
/// - `GET /settings/standing-rules` — the Standing Rules the caller's Runs
///   are created with.
/// - `PUT /settings/standing-rules` — set the team's, or go back to the
///   default.
/// - `PUT /settings/standing-rules/projects/{project_id}` — set or remove a
///   Project's own.
/// - `POST /insights/ci-quarantines` — a team's owner or admin quarantines
///   a CI job of a repository they may push to, in person.
/// - `POST /insights/ci-quarantines/release` — or releases it.
/// - `POST /internal/main-holds/clear` — an internal caller, for an
///   operator, clears a base branch's Main Hold; not in the OpenAPI
///   document.
pub fn factory_router<S, Auth, T>(state: FactoryRouterState<S, Auth>) -> Router<T>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
    T: Send + Sync + 'static,
{
    Router::new()
        .route("/flows", get(list_flows_handler::<S, Auth>))
        .route(
            "/runs",
            get(board::list_board_handler::<S, Auth>).post(create_run_handler::<S, Auth>),
        )
        .route(
            "/runs/by-pull-requests",
            post(board::pull_request_runs_handler::<S, Auth>),
        )
        .route(
            "/tasks/{task_id}/runs",
            get(list_task_runs_handler::<S, Auth>),
        )
        .route("/runs/waiting", get(list_waiting_runs_handler::<S, Auth>))
        .route("/runs/{id}", get(get_run_handler::<S, Auth>))
        .route(
            "/runs/{id}/steps/{seq}/answer",
            post(answer_gate_handler::<S, Auth>),
        )
        .route(
            "/runs/{id}/steps/{seq}/reply",
            post(reply_to_gate_handler::<S, Auth>),
        )
        .route(
            "/sessions/{session_id}/step",
            get(session_step_handler::<S, Auth>),
        )
        .route(
            "/runs/{id}/steps/{seq}/note",
            post(note_step_handler::<S, Auth>),
        )
        .route(
            "/runs/{id}/steps/{seq}/pause",
            post(pause_step_handler::<S, Auth>),
        )
        .route(
            "/runs/{id}/steps/{seq}/resume",
            post(resume_step_handler::<S, Auth>),
        )
        .route(
            "/runs/{id}/steps/{seq}/stop",
            post(stop_step_handler::<S, Auth>),
        )
        .route("/runs/{id}/cancel", post(cancel_run_handler::<S, Auth>))
        .route("/runs/{id}/retry", post(retry_run_handler::<S, Auth>))
        .route("/runs/{id}/flaw", post(flaw_run_handler::<S, Auth>))
        .route("/runs/{id}/task", put(link_task_handler::<S, Auth>))
        .route("/hand-backs", post(hand_back_handler::<S, Auth>))
        .route(
            "/settings/control-files",
            get(control_files::get_control_files_handler::<S, Auth>)
                .put(control_files::set_control_files_handler::<S, Auth>),
        )
        .route(
            "/settings/definition",
            get(definition::get_definition_handler::<S, Auth>)
                .put(definition::set_definition_handler::<S, Auth>),
        )
        .route(
            "/settings/definition/file",
            get(definition::get_definition_file_handler::<S, Auth>),
        )
        .route(
            "/settings/definition/changes",
            post(definition::propose_definition_change_handler::<S, Auth>),
        )
        .route(
            "/settings/definition/changes/check",
            post(definition::check_definition_change_handler::<S, Auth>),
        )
        .route(
            "/settings/standing-rules",
            get(standing_rules::get_standing_rules_handler::<S, Auth>)
                .put(standing_rules::set_standing_rules_handler::<S, Auth>),
        )
        .route(
            "/settings/standing-rules/projects/{project_id}",
            put(standing_rules::set_project_standing_rules_handler::<S, Auth>),
        )
        .route(
            "/insights/ci-quarantines",
            post(ci_quarantine::quarantine_ci_job_handler::<S, Auth>),
        )
        .route(
            "/insights/ci-quarantines/release",
            post(ci_quarantine::release_ci_job_handler::<S, Auth>),
        )
        .route(
            "/internal/main-holds/clear",
            post(main_hold::clear_main_hold_handler::<S, Auth>),
        )
        .with_state(state)
}

/// The Flows a Run can be started of.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FlowsResponse {
    /// The Flows, by name.
    pub flows: Vec<FlowSummary>,
}

/// Request body for creating a Run.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateRunRequest {
    /// The name of the built-in Flow the Run walks, one `GET /flows` lists.
    pub flow: String,
    /// What the Run is asked to do.
    pub request: String,
    /// The Macro task the Run works on: its task document's id.
    #[serde(default)]
    pub task_id: Option<Uuid>,
    /// The GitHub repository the Run works on, as its
    /// `https://github.com/owner/name` URL: required by a Flow that works
    /// on one, such as `code`, and refused by the others. The caller's
    /// linked GitHub account must reach it.
    #[serde(default)]
    pub repo_url: Option<String>,
}

/// Request body for linking a Run to a task.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkTaskRequest {
    /// The task document's id.
    pub task_id: Uuid,
}

/// Path params naming a task.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct TaskRunsParams {
    /// The task document's id.
    pub task_id: Uuid,
}

/// A task's Runs, as the caller sees them.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TaskRunsResponse {
    /// The caller's Runs on the task, newest first: at most the newest 50.
    pub runs: Vec<RunDetail>,
}

/// The caller's Runs that hold a place in their Queue.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WaitingRunsResponse {
    /// The caller's Runs that have not ended and wait on a Gate no one
    /// answered yet, newest first: at most the newest 50.
    pub runs: Vec<RunDetail>,
    /// The caller's Runs of a Flow with a Gate that have not ended or
    /// reached a Gate yet, newest first, at most the newest 50: each holds
    /// the place it will ask for. Cancelling one frees it.
    pub starting: Vec<RunDetail>,
}

/// Path params naming a Run.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct GetRunParams {
    /// The Run's id.
    pub id: RunId,
}

/// Path params for answering a Gate.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct AnswerGateParams {
    /// The Run's id.
    pub id: RunId,
    /// The place, among the Run's Steps, of the Step whose Gate it answers.
    pub seq: u16,
}

/// Request body for answering a Gate.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AnswerGateRequest {
    /// The answer, one the Gate offers.
    pub answer: Outcome,
    /// What the person adds, for the Run's later Steps to read.
    #[serde(default)]
    pub note: Option<String>,
    /// The Control Files the Run's branch changed unasked that the person
    /// saw and approves: required to approve a Gate of a Tampered Run, and
    /// must be exactly its `controlFiles.tampered` (empty when `unlisted`).
    #[serde(default)]
    pub approved_control_files: Option<Vec<String>>,
}

/// Request body for replying to a Gate in words.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReplyToGateRequest {
    /// The person's words, in place of one of the Gate's answers.
    pub reply: String,
}

/// Request body for a note to a running Step's agent.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NoteStepRequest {
    /// The owner's words, for the agent to take into account: at most 4096
    /// bytes, not blank.
    pub note: String,
}

/// Request body for resuming a paused Step.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResumeStepRequest {
    /// The owner's words for the agent as it goes on, if any: at most 4096
    /// bytes; a blank note is left out.
    #[serde(default)]
    pub note: Option<String>,
}

/// Request body for marking a Run flawed.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FlawRunRequest {
    /// Why its result is wrong: 1 to 4096 bytes once trimmed.
    pub reason: String,
}

/// Request body for handing back an agent Step's result.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct HandBackRequest {
    /// How the Step ended: one of the outcomes it offers.
    pub outcome: Outcome,
    /// What the agent produced, for the Run's later Steps and for people.
    pub text: String,
}

/// List the Flows the caller can start a Run of, by name: their team's
/// Factory Definition's, read at its default branch's head, when the team
/// names one (400 `unreachableDefinition` or `invalidDefinition` when it
/// does not read), else the built-in Flows.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "list_factory_flows",
    path = "/flows",
    responses(
        (status = 200, body = FlowsResponse),
        (status = 400, body = RunErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn list_flows_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
) -> Result<Json<FlowsResponse>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(FlowsResponse {
        flows: state.service.list_flows(&actor).await?,
    }))
}

/// Create a Run of a built-in Flow, owned by the caller, and start its
/// workflow. At the Queue limit of the Project it would start in, it is
/// refused (409 `queueFull`, its message counting both): the caller holds
/// as many places in their Queue as the limit, a place being a Run waiting
/// on them at a Gate or a question, or a Run of a Flow with a Gate that has
/// not reached one yet (`GET /runs/waiting` lists both). The Runs under
/// way go on.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "create_factory_run",
    path = "/runs",
    request_body = CreateRunRequest,
    responses(
        (status = 201, body = RunDetail),
        (status = 400, body = RunErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = RunErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn create_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Json(body): Json<CreateRunRequest>,
) -> Result<(StatusCode, Json<RunDetail>), FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    // Only a Run its owner created in person asks for the Control Files its
    // request names: an agent could otherwise name the file it means to
    // weaken.
    let created_in_person = is_in_person(&caller.authorization, &headers);
    let acting = acting_agent(&caller.authorization, &headers);
    // The id is lowercase, whatever case the caller named it in: a
    // `MacroUserIdStr` lowercases what it parses, as `"User".id` is stored.
    let actor = caller.authorization.user.macro_user_id;
    let created = state
        .service
        .create_run(
            actor,
            body.flow,
            body.request,
            body.task_id,
            body.repo_url,
            created_in_person,
            acting,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

/// List the caller's Runs on a task they can see, newest first, each with
/// the Steps it took. Other people's Runs on the task are not listed.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "list_factory_task_runs",
    path = "/tasks/{task_id}/runs",
    params(TaskRunsParams),
    responses(
        (status = 200, body = TaskRunsResponse),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(task_id = %params.task_id))]
pub async fn list_task_runs_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<TaskRunsParams>,
) -> Result<Json<TaskRunsResponse>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    let runs = state.service.list_task_runs(&actor, params.task_id).await?;
    Ok(Json(TaskRunsResponse { runs }))
}

/// List the caller's Runs that wait on them at a Gate no one answered
/// yet, a Flow's Gate no Standing Rule answered or an agent's question,
/// newest first, each with the Steps it took; and, apart, those that hold
/// a place in their Queue before asking anything (`starting`).
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "list_factory_waiting_runs",
    path = "/runs/waiting",
    responses(
        (status = 200, body = WaitingRunsResponse),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn list_waiting_runs_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
) -> Result<Json<WaitingRunsResponse>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    let runs = state.service.list_waiting_runs(&actor).await?;
    let starting = state.service.list_starting_runs(&actor).await?;
    Ok(Json(WaitingRunsResponse { runs, starting }))
}

/// Read one of the caller's Runs and the Steps it took.
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_run",
    path = "/runs/{id}",
    params(GetRunParams),
    responses(
        (status = 200, body = RunDetail),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn get_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<GetRunParams>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(state.service.get_run(&actor, params.id).await?))
}

/// Answer, as the caller, the Gate one of their Runs waits on. The Run
/// goes on once its workflow took the answer; reading the Run shows the
/// answer once it is recorded. A Gate asks a person: the owner's own
/// signed-in session answers it, and an agent session they work with
/// (such as the Orchestrator in Macro chat), from its code, answers only
/// what a Standing Rule could: never an agent's question, a Gate its Flow
/// keeps for a person, approving a merge or a Tampered Run, nor an
/// approving answer the Run's Orchestrator Definition does not allow
/// (403 `reservedForAPerson`); only from a chat, never from a routine's
/// run or a factory Step's own session. A Gate answered since is a
/// conflict (409). Any other caller is refused (403).
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "answer_factory_gate",
    path = "/runs/{id}/steps/{seq}/answer",
    params(AnswerGateParams),
    request_body = AnswerGateRequest,
    responses(
        (status = 202),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = RunErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id, seq = params.seq))]
pub async fn answer_gate_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AnswerGateParams>,
    Json(body): Json<AnswerGateRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    // An agent works as the Run's owner, so its code could otherwise
    // approve the plan it wrote, or the Control Files it changed: the
    // service holds an agent's answer to what a rule could give.
    let (actor, answerer) = answerer(caller.authorization, &headers)?;
    state
        .service
        .answer_gate(
            actor,
            params.id,
            params.seq,
            body.answer,
            body.note,
            body.approved_control_files,
            answerer,
        )
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// Reply, as the caller, in words to the Gate one of their Runs waits on.
/// The factory reads the words as one of the Gate's answers, kept as its
/// note, or asks back on the Gate, which then still waits; an agent's
/// question takes them as the answer. Reading the Run shows which. Only
/// the owner's own signed-in session replies (403 otherwise).
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "reply_to_factory_gate",
    path = "/runs/{id}/steps/{seq}/reply",
    params(AnswerGateParams),
    request_body = ReplyToGateRequest,
    responses(
        (status = 202),
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
#[tracing::instrument(skip_all, fields(run_id = %params.id, seq = params.seq))]
pub async fn reply_to_gate_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AnswerGateParams>,
    Json(body): Json<ReplyToGateRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    // As for an answer: an agent works as the Run's owner, and its words
    // would answer its own Gate.
    let actor = in_person(caller.authorization, &headers)?;
    state
        .service
        .reply_to_gate(actor, params.id, params.seq, body.reply)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// Path params naming an agent session.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct SessionStepParams {
    /// The agent session's id.
    pub session_id: Uuid,
}

/// The Step of one of the caller's Runs that works, or worked, in an agent
/// session: the session's view tells the person to steer it from its Run.
/// A session no Step of theirs works in is not found (404).
#[utoipa::path(
    get,
    tag = "factory",
    operation_id = "get_factory_session_step",
    path = "/sessions/{session_id}/step",
    params(SessionStepParams),
    responses(
        (status = 200, body = SessionStepRef),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(session_id = %params.session_id))]
pub async fn session_step_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    Path(params): Path<SessionStepParams>,
) -> Result<Json<SessionStepRef>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(
        state
            .service
            .session_step(&actor, params.session_id)
            .await?,
    ))
}

/// Send the agent of the running Step `seq` of one of the caller's Runs a
/// note, from its Step Terminal. The agent reads it once its turn ends, or
/// at once when it works on none, and the Step waits for it to answer: it
/// ends with the agent's latest hand back. Only the Run's owner, in their
/// own signed-in session, steers a Step (403 otherwise). A Step that ended
/// (409), or takes no steering now: paused, asked to stop, between its
/// agent's turns, or with no agent session (409). A blank note, or one
/// past 4096 bytes, is refused (400); a session that did not take it is
/// 502.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "note_factory_step",
    path = "/runs/{id}/steps/{seq}/note",
    params(AnswerGateParams),
    request_body = NoteStepRequest,
    responses(
        (status = 202),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id, seq = params.seq))]
pub async fn note_step_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AnswerGateParams>,
    Json(body): Json<NoteStepRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    steer_step(
        &state,
        caller.authorization,
        &headers,
        params,
        TerminalRequest::Note(body.note),
    )
    .await
}

/// Pause the running Step `seq` of one of the caller's Runs: its agent's
/// turn stops, and the Step holds until it is resumed or stopped. The time
/// it is paused does not count against the Step's wait for its agent, but
/// a Step still paused when its two hours run out ends `failed`. As for a
/// note: only the owner in person (403), a Step that ended or takes no
/// steering now, such as one already paused (409).
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "pause_factory_step",
    path = "/runs/{id}/steps/{seq}/pause",
    params(AnswerGateParams),
    responses(
        (status = 202),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id, seq = params.seq))]
pub async fn pause_step_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AnswerGateParams>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    steer_step(
        &state,
        caller.authorization,
        &headers,
        params,
        TerminalRequest::Pause,
    )
    .await
}

/// Resume the paused Step `seq` of one of the caller's Runs: its agent is
/// told to go on where it left off, with the owner's note if they wrote
/// one, and the Step follows that turn. As for a note: only the owner in
/// person (403), a Step that ended or is not paused (409), a note past 4096
/// bytes (400), a session that did not take it (502), which leaves the
/// Step paused.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "resume_factory_step",
    path = "/runs/{id}/steps/{seq}/resume",
    params(AnswerGateParams),
    request_body = ResumeStepRequest,
    responses(
        (status = 202),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id, seq = params.seq))]
pub async fn resume_step_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AnswerGateParams>,
    Json(body): Json<ResumeStepRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    steer_step(
        &state,
        caller.authorization,
        &headers,
        params,
        TerminalRequest::Resume(body.note),
    )
    .await
}

/// Stop the running Step `seq` of one of the caller's Runs: its agent's
/// turn stops, and the Step ends `failed`, its session stopped for good;
/// the Flow routes the failure. As for a note: only the owner in person
/// (403), a Step that ended or takes no steering now (409).
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "stop_factory_step",
    path = "/runs/{id}/steps/{seq}/stop",
    params(AnswerGateParams),
    responses(
        (status = 202),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id, seq = params.seq))]
pub async fn stop_step_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<AnswerGateParams>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    steer_step(
        &state,
        caller.authorization,
        &headers,
        params,
        TerminalRequest::Stop,
    )
    .await
}

/// Steer a Step for its owner in person. An agent works as the Run's owner,
/// and its code could otherwise steer the Step it works on, or another.
async fn steer_step<S, Auth>(
    state: &FactoryRouterState<S, Auth>,
    authorization: UserOrInternalAuthorization,
    headers: &HeaderMap,
    params: AnswerGateParams,
    request: TerminalRequest,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(authorization, headers)?;
    state
        .service
        .steer_step(actor, params.id, params.seq, request)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// The caller, when they are the person in their own signed-in session:
/// not an internal caller acting for them, nor an API key or token, nor an
/// agent's program.
fn in_person(
    authorization: UserOrInternalAuthorization,
    headers: &HeaderMap,
) -> Result<MacroUserIdStr<'static>, FactoryError> {
    if !is_in_person(&authorization, headers) {
        return Err(FactoryError::NotAPerson);
    }
    Ok(authorization.user.macro_user_id)
}

/// Who answers a Gate: the person in their own signed-in session, or an
/// agent session they chat with acting for them from its code, as the
/// correlation layer validated the session against the call's credential.
/// An internal caller, an API key or token, a routine's run, and a
/// background run held to a scope answer nothing.
fn answerer(
    authorization: UserOrInternalAuthorization,
    headers: &HeaderMap,
) -> Result<(MacroUserIdStr<'static>, Answerer), FactoryError> {
    if is_in_person(&authorization, headers) {
        return Ok((authorization.user.macro_user_id, Answerer::InPerson));
    }
    let session = agent_correlation::context::current_session();
    let Some(session_id) = session.filter(|_| authorization.caller == UserOrInternalCaller::User)
    else {
        return Err(FactoryError::NotAPerson);
    };
    // Only a chat with the person: a routine's run, signed for it or
    // carrying its id, and a background run held to a scope answer
    // nothing for them.
    let routine = agent_correlation::context::current_routine().is_some()
        || authorization.user.routine_id.is_some();
    if routine || agent_correlation::context::current_scope().is_some() {
        return Err(PersonOnly::NotAChat.into());
    }
    Ok((
        authorization.user.macro_user_id,
        Answerer::Agent { session_id },
    ))
}

/// The agent a call acts as for the caller, for Standing Roles to check:
/// none for the person in their own signed-in session and for an internal
/// caller. A program in an agent session acts as that session, as the
/// correlation layer validated it against the call's credential. A program
/// in no session whose credential is signed for a bot (a self-hosted
/// agent's code-mode call, a routine's run for an agent) acts as that bot:
/// the signature names it, so it cannot be forged. A program signed only
/// for the person (their own API key or token, a routine running as them
/// with no agent) is the person acting, which Standing Roles do not
/// govern. So is a public MCP credential in no session: Macro signs the
/// person's own MCP client or API key, directly or through code mode, for
/// the Macro bot only to label its posts (`signed_for_the_person`).
pub(crate) fn acting_agent(
    authorization: &UserOrInternalAuthorization,
    headers: &HeaderMap,
) -> Option<ActingAgent> {
    if is_in_person(authorization, headers) || authorization.caller != UserOrInternalCaller::User {
        return None;
    }
    program_agent(&authorization.user)
}

/// The agent a program's call acts as, for a caller never in person (a
/// routine's run): its agent session, or else the bot its credential is
/// signed for, unless that only labels the person's own client.
pub(crate) fn program_agent(
    user: &macro_authorization::MacroUserAuthentication,
) -> Option<ActingAgent> {
    agent_correlation::context::current_session()
        .map(ActingAgent::Session)
        .or_else(|| {
            user.agent_id
                .filter(|_| !signed_for_the_person(user))
                .map(ActingAgent::Bot)
        })
}

/// Whether a credential in no session names the Macro bot only as the
/// label of the person's own client: public MCP signs the person's MCP
/// client or API key for `MACRO_AI_BOT_ID`, with no routine, where a
/// self-hosted agent's call is signed for its own bot.
fn signed_for_the_person(user: &macro_authorization::MacroUserAuthentication) -> bool {
    user.agent_id == Some(bot_id::MACRO_AI_BOT_ID) && user.routine_id.is_none()
}

/// Whether the caller is the person in their own signed-in session: not an
/// internal caller, an API key or token, an agent's or a routine's program.
/// An agent's account signs in as a person does; the service, which reads
/// accounts, refuses it.
pub(crate) fn is_in_person(
    authorization: &UserOrInternalAuthorization,
    headers: &HeaderMap,
) -> bool {
    authorization.caller == UserOrInternalCaller::User
        && authorization.user.is_human_session()
        && !is_program_run(authorization.user.attribution(), headers)
}

/// Cancel one of the caller's Runs. Its running Step or Gate stops, and
/// reading the Run shows it `cancelled` once its workflow recorded that.
/// Cancelling a cancelled Run again is accepted and changes nothing.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "cancel_factory_run",
    path = "/runs/{id}/cancel",
    params(GetRunParams),
    responses(
        (status = 202),
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
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn cancel_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<GetRunParams>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let acting = acting_agent(&caller.authorization, &headers);
    let in_person = is_in_person(&caller.authorization, &headers);
    let actor = caller.authorization.user.macro_user_id;
    state
        .service
        .cancel_run(&actor, params.id, acting, in_person)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

/// Retry one of the caller's Runs that ended `failed`, `infrastructure` or
/// `blocked`: a new Run of the same task, Flow and Factory Definition
/// commit, on the same branch, that keeps the Steps the Run took before the
/// entry it stopped at, with their outputs, and starts there; its
/// `retry.retriedFrom` names the Run it retries. A Run is retried once: a
/// second retry, a running Run, a Run that ended otherwise, a Benchmark
/// trial and a self-improvement Run are refused (409 `notRetriable`, and
/// `alreadyRetried` for a second retry), as is a retry past the caller's
/// Queue limit (`queueFull`). An agent acting for the owner
/// needs a Standing Role that allows `start`, as a new Run does.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "retry_factory_run",
    path = "/runs/{id}/retry",
    params(GetRunParams),
    responses(
        (status = 201, body = RunDetail),
        (status = 401, body = ErrorResponse),
        (status = 403, body = RunErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = RunErrorResponse),
        (status = 500, body = ErrorResponse),
        (status = 502, body = ErrorResponse),
        (status = 503, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn retry_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<GetRunParams>,
) -> Result<(StatusCode, Json<RunDetail>), FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let acting = acting_agent(&caller.authorization, &headers);
    let in_person = is_in_person(&caller.authorization, &headers);
    let actor = caller.authorization.user.macro_user_id;
    let detail = state
        .service
        .retry_run(&actor, params.id, acting, in_person)
        .await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

/// Mark one of the caller's Runs flawed: it ended `success`, and they
/// found its result wrong. The Run then reads `flawed`, with who marked it,
/// why and when. Marking a flawed Run again keeps its first mark. Only the
/// Run's owner marks it, in their own signed-in session: an agent's code,
/// an API key or token and an internal caller are refused (403).
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "flaw_factory_run",
    path = "/runs/{id}/flaw",
    params(GetRunParams),
    request_body = FlawRunRequest,
    responses(
        (status = 200, body = RunDetail),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn flaw_run_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<GetRunParams>,
    Json(body): Json<FlawRunRequest>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let actor = in_person(caller.authorization, &headers)?;
    let detail = state
        .service
        .mark_run_flawed(&actor, params.id, &body.reason)
        .await?;
    Ok(Json(detail))
}

/// Link one of the caller's Runs to a Macro task they can see,
/// replacing any task it had.
#[utoipa::path(
    put,
    tag = "factory",
    operation_id = "link_factory_run_task",
    path = "/runs/{id}/task",
    params(GetRunParams),
    request_body = LinkTaskRequest,
    responses(
        (status = 200, body = RunDetail),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 404, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all, fields(run_id = %params.id))]
pub async fn link_task_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOrInternal>,
    headers: HeaderMap,
    Path(params): Path<GetRunParams>,
    Json(body): Json<LinkTaskRequest>,
) -> Result<Json<RunDetail>, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    let acting = acting_agent(&caller.authorization, &headers);
    let actor = caller.authorization.user.macro_user_id;
    Ok(Json(
        state
            .service
            .link_task(&actor, params.id, body.task_id, acting)
            .await?,
    ))
}

/// Hand back the result of the factory Step the calling agent session works
/// on. The session's latest hand back is the Step's result once its turn
/// ends.
#[utoipa::path(
    post,
    tag = "factory",
    operation_id = "hand_back_factory_step",
    path = "/hand-backs",
    request_body = HandBackRequest,
    responses(
        (status = 204),
        (status = 400, body = ErrorResponse),
        (status = 401, body = ErrorResponse),
        (status = 403, body = ErrorResponse),
        (status = 409, body = ErrorResponse),
        (status = 500, body = ErrorResponse),
    )
)]
#[tracing::instrument(skip_all)]
pub async fn hand_back_handler<S, Auth>(
    State(state): State<FactoryRouterState<S, Auth>>,
    caller: MacroAuthorizationExtractor<Auth, UserOnly>,
    Json(body): Json<HandBackRequest>,
) -> Result<StatusCode, FactoryError>
where
    S: FactoryService,
    Auth: MacroAuthorizationService,
{
    // The session behind the code-mode credential, validated against it by
    // the correlation layer; no other caller names one.
    let session_id =
        agent_correlation::context::current_session().ok_or(FactoryError::NotAStepSession)?;
    let hand_back = HandBack {
        outcome: body.outcome,
        output: body.text,
    };
    state
        .service
        .hand_back(session_id, caller.authorization.macro_user_id, hand_back)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

impl FactoryError {
    /// The status the API answers this error with.
    fn status(&self) -> StatusCode {
        match self {
            FactoryError::InvalidFlow(_)
            | FactoryError::EmptyRequest
            | FactoryError::HalfCursor
            | FactoryError::TooManyPullRequests { .. }
            | FactoryError::RequestTooLarge { .. }
            | FactoryError::NoteTooLarge { .. }
            | FactoryError::EmptyNote
            | FactoryError::FlawReasonSize { .. }
            | FactoryError::InvalidScorer(_)
            | FactoryError::UnknownTask
            | FactoryError::NotATask
            | FactoryError::UnknownUser
            | FactoryError::UnknownFlow
            | FactoryError::RepositoryRequired
            | FactoryError::RepositoryNotUsed
            | FactoryError::InvalidRepository
            | FactoryError::InvalidCiJob
            | FactoryError::UnreachableRepository
            | FactoryError::NoPushAccess
            | FactoryError::UnreachableDefinition
            | FactoryError::InvalidDefinition(_)
            | FactoryError::InvalidDefinitionChange(_)
            | FactoryError::NotADefinitionFile(_)
            | FactoryError::AnswerNotOffered { .. }
            | FactoryError::NotAStepSession
            | FactoryError::OutcomeNotOffered { .. }
            | FactoryError::HandBackTooLarge { .. }
            | FactoryError::QuestionSize { .. }
            | FactoryError::AnswerMissing
            | FactoryError::ControlFilesNotApproved
            | FactoryError::UnknownProject
            | FactoryError::InvalidQueueLimit { .. }
            | FactoryError::InvalidTokenBudget { .. }
            | FactoryError::InvalidControlFiles(_)
            | FactoryError::NoTeam
            | FactoryError::InvalidAutomation(_)
            | FactoryError::InvalidFiring(_)
            | FactoryError::TriggerRefused(_) => StatusCode::BAD_REQUEST,
            FactoryError::BadSignature => StatusCode::UNAUTHORIZED,
            FactoryError::NotTheOwner
            | FactoryError::NotAPerson
            | FactoryError::NotATeamAdmin
            | FactoryError::ReservedForAPerson(_)
            | FactoryError::NotInStandingRole { .. }
            | FactoryError::NotTheAutomationOwner
            | FactoryError::NotARoutine => StatusCode::FORBIDDEN,
            FactoryError::NotFound | FactoryError::TaskNotFound => StatusCode::NOT_FOUND,
            FactoryError::RunEnded
            | FactoryError::NotRetriable
            | FactoryError::AlreadyRetried
            | FactoryError::QueueFull { .. }
            | FactoryError::AnswerRefused(_)
            | FactoryError::StepEnded
            | FactoryError::NotSteerable(_)
            | FactoryError::TaskBusy
            | FactoryError::NotSucceeded
            | FactoryError::FiringRepeated
            | FactoryError::DefinitionNamed
            | FactoryError::NoDefinition
            | FactoryError::DefinitionChangeNotOpened(_)
            | FactoryError::DefinitionChangeClosed(_) => StatusCode::CONFLICT,
            FactoryError::TriggerUnavailable | FactoryError::SessionUnreachable => {
                StatusCode::BAD_GATEWAY
            }
            FactoryError::Upstream(upstream) => upstream.status_code(),
            FactoryError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Why a request was refused, where a client acts on the cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum RunErrorCode {
    /// The caller's linked GitHub account cannot reach the repository, or
    /// none is linked, or the repository has no branch yet.
    UnreachableRepository,
    /// The caller's linked GitHub account may read the repository but not
    /// push to it.
    NoPushAccess,
    /// The Run is Tampered, and approving its Gate needs
    /// `approvedControlFiles` listing its Tampered files.
    ControlFilesNotApproved,
    /// The caller holds as many places in their Queue as the Queue limit of
    /// the Project the Run would start in: Runs waiting on them at a Gate or
    /// a question, and Runs of a Flow with a Gate that have not reached one
    /// yet. No new Run starts until they answer some, or cancel one.
    QueueFull,
    /// The Run cannot be retried: it has not ended, it ended other than
    /// `failed`, `infrastructure` or `blocked`, it is a Benchmark trial or
    /// a self-improvement Run, or it keeps no Flow to start again.
    NotRetriable,
    /// The Run was retried already: its `retriedBy` names the retry, which
    /// is the one to retry next.
    AlreadyRetried,
    /// An agent answering for the Run's owner tried what the factory keeps
    /// for the owner in person.
    ReservedForAPerson,
    /// An agent acting for the caller tried an act no Standing Role its bot
    /// fills allows; the message names the Standing Roles that allow it.
    NotInStandingRole,
    /// The caller's linked GitHub account cannot read the repository of
    /// their team's Factory Definition.
    UnreachableDefinition,
    /// Their team's Factory Definition does not read; the message says
    /// which file and why.
    InvalidDefinition,
    /// Their team's Factory Definition sets this: it changes by pull
    /// request.
    DefinitionNamed,
    /// Their team names no Factory Definition, so there is none to change.
    NoDefinition,
    /// The proposed change to their team's Factory Definition is refused:
    /// the Definition does not read with it, or the change itself is
    /// wrong; the message says which file and why.
    InvalidDefinitionChange,
    /// GitHub did not create the change's branch or open its pull request
    /// (a rule of the repository, or its branch already there with no pull
    /// request); the message says why.
    DefinitionChangeNotOpened,
    /// The change's pull request was closed without merging; it is not
    /// opened again.
    DefinitionChangeClosed,
    /// The path names no file a Factory Definition holds.
    NotADefinitionFile,
}

/// What a refused or failed request answers: a message, and for a refusal
/// a client acts on, its code.
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunErrorResponse {
    /// What went wrong, in words.
    pub message: String,
    /// Why it was refused, when a client acts on the cause.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<RunErrorCode>,
}

impl FactoryError {
    /// The code a client acts on, if this error has one.
    fn code(&self) -> Option<RunErrorCode> {
        match self {
            FactoryError::UnreachableRepository => Some(RunErrorCode::UnreachableRepository),
            FactoryError::NoPushAccess => Some(RunErrorCode::NoPushAccess),
            FactoryError::ControlFilesNotApproved => Some(RunErrorCode::ControlFilesNotApproved),
            FactoryError::QueueFull { .. } => Some(RunErrorCode::QueueFull),
            FactoryError::NotRetriable => Some(RunErrorCode::NotRetriable),
            FactoryError::AlreadyRetried => Some(RunErrorCode::AlreadyRetried),
            FactoryError::ReservedForAPerson(_) => Some(RunErrorCode::ReservedForAPerson),
            FactoryError::NotInStandingRole { .. } => Some(RunErrorCode::NotInStandingRole),
            FactoryError::UnreachableDefinition => Some(RunErrorCode::UnreachableDefinition),
            FactoryError::InvalidDefinition(_) => Some(RunErrorCode::InvalidDefinition),
            FactoryError::DefinitionNamed => Some(RunErrorCode::DefinitionNamed),
            FactoryError::NoDefinition => Some(RunErrorCode::NoDefinition),
            FactoryError::InvalidDefinitionChange(_) => Some(RunErrorCode::InvalidDefinitionChange),
            FactoryError::DefinitionChangeNotOpened(_) => {
                Some(RunErrorCode::DefinitionChangeNotOpened)
            }
            FactoryError::DefinitionChangeClosed(_) => Some(RunErrorCode::DefinitionChangeClosed),
            FactoryError::NotADefinitionFile(_) => Some(RunErrorCode::NotADefinitionFile),
            _ => None,
        }
    }
}

impl IntoResponse for FactoryError {
    fn into_response(self) -> axum::response::Response {
        // Every error is logged once, here; the handlers' spans do not log
        // it again.
        let status_code = self.status();
        let code = self.code();
        let message = match self {
            FactoryError::Upstream(upstream) => {
                tracing::warn!(error = %upstream, "the factory's workflow service failed");
                return upstream.into_response();
            }
            FactoryError::Internal(_) => {
                tracing::error!(error = ?self, "factory internal server error");
                "internal server error".to_string()
            }
            error => error.to_string(),
        };

        (status_code, Json(RunErrorResponse { message, code })).into_response()
    }
}

/// The Run API's OpenAPI document.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_flows_handler,
        list_task_runs_handler,
        list_waiting_runs_handler,
        board::list_board_handler,
        board::pull_request_runs_handler,
        create_run_handler,
        get_run_handler,
        answer_gate_handler,
        reply_to_gate_handler,
        session_step_handler,
        note_step_handler,
        pause_step_handler,
        resume_step_handler,
        stop_step_handler,
        cancel_run_handler,
        retry_run_handler,
        flaw_run_handler,
        link_task_handler,
        hand_back_handler,
        control_files::get_control_files_handler,
        control_files::set_control_files_handler,
        definition::get_definition_handler,
        definition::set_definition_handler,
        definition::propose_definition_change_handler,
        definition::get_definition_file_handler,
        definition::check_definition_change_handler,
        standing_rules::get_standing_rules_handler,
        standing_rules::set_standing_rules_handler,
        standing_rules::set_project_standing_rules_handler,
        ci_quarantine::quarantine_ci_job_handler,
        ci_quarantine::release_ci_job_handler
    ),
    components(schemas(
        FlowsResponse,
        FlowSummary,
        TaskRunsResponse,
        WaitingRunsResponse,
        BoardPage,
        BoardRun,
        PullRequestRuns,
        PullRequestRun,
        board::PullRequestUrls,
        BoardNext,
        BoardState,
        BoardScope,
        CreateRunRequest,
        AnswerGateRequest,
        ReplyToGateRequest,
        NoteStepRequest,
        ResumeStepRequest,
        SessionStepRef,
        LinkTaskRequest,
        HandBackRequest,
        FlawRunRequest,
        RunDetail,
        Run,
        RunEnd,
        RunOutcome,
        RunFlaw,
        RunRetry,
        FlawMarker,
        Step,
        StepEnd,
        StepTerminal,
        StepRuntime,
        Steer,
        SteerAction,
        Gate,
        GateAnswer,
        AskedBack,
        GateModelReading,
        StandInAnswer,
        ControlFileChanges,
        ControlFilesSettings,
        control_files::SetControlFilesRequest,
        DefinitionSettings,
        definition::SetDefinitionRequest,
        definition::ProposeDefinitionChangeRequest,
        definition::CheckDefinitionChangeRequest,
        DefinitionFile,
        DefinitionChangeCheck,
        DefinitionRead,
        DefinitionContents,
        DefinitionFlow,
        DefinitionRole,
        DefinitionScorer,
        DefinitionPullRequest,
        RunDefinition,
        StandingRulesSettings,
        OrchestratorDefinition,
        StandInKinds,
        OrchestratorKinds,
        ProjectRules,
        RunRules,
        RulesSource,
        StandingRule,
        standing_rules::SetStandingRulesRequest,
        ci_quarantine::CiJobRequest,
        CiQuarantineChange,
        MacroUserIdStr,
        EntryId,
        Outcome,
        RunId,
        ErrorResponse,
        RunErrorResponse,
        RunErrorCode,
    )),
    tags((name = "factory", description = "Runs of the software factory's Flows")),
)]
pub struct ApiDoc;
