//! Composition root of the factory service: the Run API, and the Temporal
//! worker that walks each Run's Flow.
// The Run API's handler futures nest every port of the factory service.
#![recursion_limit = "256"]

mod config;
mod health;

use std::sync::Arc;
use std::time::Duration;

use agent_correlation::inbound::CorrelationState;
use agent_harness_service_client::routine_sessions::AgentSessionClient;
use ai_usage::outbound::pg_session_usage::PgSessionUsageReader;
use authentication_service_client::AuthServiceClient;
use axum::Router;
use config::Config;
use document_storage_service_client::DocumentStorageServiceClient;
use entity_access::domain::service::EntityAccessServiceImpl;
use entity_access::outbound::PgAccessRepository;
use factory_engine::domain::agents::AgentDefinition;
use factory_engine::domain::built_in_steps::BuiltInSteps;
use factory_engine::domain::insights::InsightsServiceImpl;
use factory_engine::domain::main_hold::MainWatchServiceImpl;
use factory_engine::domain::ports::StepAgent;
use factory_engine::domain::pull_request_steps::PullRequestSteps;
use factory_engine::domain::service::{FactoryServiceImpl, StepServiceImpl};
use factory_engine::domain::session_agent::SessionAgent;
use factory_engine::inbound::http::{ApiDoc, FactoryRouterState, factory_router};
use factory_engine::inbound::insights::{InsightsApiDoc, InsightsRouterState, insights_router};
use factory_engine::outbound::auth_user_directory::AuthUserDirectory;
use factory_engine::outbound::document_hold_tasks::DocumentHoldTasks;
use factory_engine::outbound::document_task_directory::DocumentTaskDirectory;
use factory_engine::outbound::github_pull_requests::GithubPullRequests;
use factory_engine::outbound::local_agent::LocalAgent;
use factory_engine::outbound::temporal_main_watches::TemporalMainWatches;
use factory_engine::outbound::temporal_run_workflows::TemporalRunWorkflows;
use factory_records::outbound::pg_run_repository::PgRunRepository;
use macro_auth::middleware::decode_jwt::JwtValidationArgs;
use macro_authorization::{
    InternalAuthConfig, MacroAuthJwtValidator, MacroAuthorizationServiceImpl,
    MacroAuthorizationState, NoBotAuthorizer, PgUserApiKeyAuthorizationRepo,
    PgUserApiKeyAuthorizer,
};
use macro_entrypoint::{MacroEntrypoint, shutdown_signal};
use macro_service_urls::{AgentHarnessServiceUrl, AuthServiceUrl, DocumentStorageServiceUrl};
use macro_tower_layers::MacroRequestIdAndTracingLayer;
use sqlx::postgres::PgPoolOptions;
use temporalio_client::{Client, ClientOptions, ConnectionOptions};
use temporalio_sdk::Runtime;
use tokio_util::sync::CancellationToken;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// Path prefix the gateway forwards unchanged.
const GATEWAY_PATH_PREFIX: &str = "/factory";

/// The Temporal namespace the Runs' workflows run in.
const TEMPORAL_NAMESPACE: &str = "default";

/// How long a request may take before its span is logged as slow.
const SLOW_REQUEST: Duration = Duration::from_millis(200);

#[tokio::main]
async fn main() -> Result<(), rootcause::Report> {
    let entrypoint = MacroEntrypoint::default().init();
    let result = run().await;
    entrypoint.shutdown();
    result
}

async fn run() -> Result<(), rootcause::Report> {
    let config = macro_config::ConfigLoader::load::<Config>()?;

    let db = PgPoolOptions::new()
        .min_connections(1)
        .max_connections(10)
        .connect(config.database_url.as_ref())
        .await?;
    let repo = PgRunRepository::new(db.clone());

    let temporal_address = url::Url::parse(config.temporal_address.as_ref())?;
    let client = Client::connect(
        ConnectionOptions::new(temporal_address)
            .identity("factory_service".to_string())
            .build(),
        ClientOptions::new(TEMPORAL_NAMESPACE).build(),
    )
    .await?;

    let secretsmanager_client = secretsmanager_client::SecretsManager::new(
        aws_sdk_secretsmanager::Client::new(&macro_aws_config::get_macro_aws_config().await),
    );
    let jwt_args =
        JwtValidationArgs::new_with_secret_manager(config.environment, &secretsmanager_client)
            .await?;
    // People call the Run API with their own login or user API key, and
    // internal services for an acting user. Bots are refused.
    let authorization_state =
        MacroAuthorizationState::new(Arc::new(MacroAuthorizationServiceImpl::new(
            MacroAuthJwtValidator::new(jwt_args.clone()),
            InternalAuthConfig {
                api_key: config.internal_api_key.to_string(),
                default_user_id: None,
            },
            NoBotAuthorizer,
            PgUserApiKeyAuthorizer::new(PgUserApiKeyAuthorizationRepo::new(db.clone())),
        )));

    // The authentication service says which accounts exist, and holds the
    // GitHub account each person linked, as whom the Steps act on GitHub.
    let auth = AuthServiceClient::new(
        config.authentication_service_secret_key.clone(),
        AuthServiceUrl::new()?.to_string(),
    );
    let github = GithubPullRequests::new(auth.clone());
    let documents = DocumentStorageServiceClient::new(
        config.document_storage_service_auth_key.clone(),
        DocumentStorageServiceUrl::new()?.to_string(),
    );
    let parts = Parts {
        port: config.port,
        repo: repo.clone(),
        usage: PgSessionUsageReader::new(db.clone()),
        users: AuthUserDirectory::new(auth),
        github: github.clone(),
        tasks: DocumentTaskDirectory::new(
            EntityAccessServiceImpl::new(PgAccessRepository::new(db.clone())),
            documents.clone(),
        ),
        hold_tasks: DocumentHoldTasks::new(documents),
        client,
        authorization_state,
        correlation: CorrelationState::new(db, jwt_args),
    };

    let pull_requests = PullRequestSteps::new(
        github,
        repo.clone(),
        TemporalMainWatches::new(parts.client.clone()),
    );

    // The agent Steps run as the Run's owner in sessions of the Agents each
    // Role takes, which must fill every Role a built-in Flow names. Without
    // them, they run on the local stand-in, which does no work.
    let agents =
        AgentDefinition::configured(config.factory_agents, config.factory_agent_id.as_deref())?;
    match agents {
        Some(agents) => {
            let sessions = AgentSessionClient::new(
                AgentHarnessServiceUrl::new()?.as_ref(),
                config.internal_api_key.as_ref(),
            )
            .map_err(|error| rootcause::report!("{error:?}"))?;
            let agent = SessionAgent::new(sessions, repo.clone(), agents);
            serve(parts, BuiltInSteps::new(agent, pull_requests)).await
        }
        None => {
            tracing::warn!("FACTORY_AGENTS is unset: agent steps run on the local stand-in");
            serve(parts, BuiltInSteps::new(LocalAgent, pull_requests)).await
        }
    }
}

/// What serving takes besides the Steps.
struct Parts {
    port: u16,
    repo: PgRunRepository,
    usage: PgSessionUsageReader,
    users: AuthUserDirectory,
    github: GithubPullRequests<AuthServiceClient>,
    tasks: DocumentTaskDirectory<EntityAccessServiceImpl<PgAccessRepository>>,
    hold_tasks: DocumentHoldTasks,
    client: Client,
    authorization_state: MacroAuthorizationState<Authorization>,
    correlation: CorrelationState,
}

/// The authorization service the Run API checks callers with.
type Authorization = MacroAuthorizationServiceImpl<MacroAuthJwtValidator>;

/// Serve the Run API and run the worker over `steps`, until a shutdown
/// signal or until either stops.
async fn serve<A: StepAgent + Clone>(
    parts: Parts,
    steps: BuiltInSteps<
        A,
        GithubPullRequests<AuthServiceClient>,
        PgRunRepository,
        TemporalMainWatches,
    >,
) -> Result<(), rootcause::Report> {
    let runtime = Runtime::from_current_tokio(Default::default())?;
    let worker = factory_engine::inbound::temporal::worker(
        &runtime,
        parts.client.clone(),
        StepServiceImpl::new(parts.repo.clone(), steps.clone()),
        MainWatchServiceImpl::new(parts.repo.clone(), parts.github.clone(), parts.hold_tasks),
    )?;

    let insights = Arc::new(InsightsServiceImpl::new(parts.repo.clone(), parts.usage));
    let service = Arc::new(FactoryServiceImpl::new(
        parts.repo,
        TemporalRunWorkflows::new(parts.client),
        steps,
        parts.users,
        parts.tasks,
        parts.github,
    ));
    let api = factory_router(FactoryRouterState::new(
        service,
        parts.authorization_state.clone(),
    ))
    .merge(insights_router(InsightsRouterState::new(
        insights,
        parts.authorization_state,
    )))
    .merge(health::router());
    let mut openapi = ApiDoc::openapi();
    openapi.merge(InsightsApiDoc::openapi());
    // The correlation layer finds the agent session behind a code-mode
    // credential, and holds a scoped session to its scope.
    let app = traced(mount_at_root_and_prefix(api))
        .merge(SwaggerUi::new("/docs").url("/api-doc/openapi.json", openapi))
        .layer(axum::middleware::from_fn_with_state(
            parts.correlation,
            agent_correlation::inbound::propagate,
        ));

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", parts.port)).await?;
    tracing::info!(port = parts.port, "factory service is up");

    let shutdown = CancellationToken::new();
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown_signal().await;
            shutdown.cancel();
        }
    });

    // The API and the worker stop together: on a shutdown signal, or when
    // either of them stops on its own. The worker gives running Steps a
    // grace period, then cancels them, before it returns.
    let serving = async {
        let served = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.clone().cancelled_owned())
            .await;
        shutdown.cancel();
        served
    };
    let working = async {
        let worked = worker.run_until(shutdown.clone().cancelled_owned()).await;
        shutdown.cancel();
        worked
    };
    let (served, worked) = tokio::join!(serving, working);
    served?;
    worked?;
    Ok(())
}

/// Wraps the API in the request id, server span, CORS and compression layers.
fn traced(api: Router) -> Router {
    api.layer(
        ServiceBuilder::new()
            .layer(MacroRequestIdAndTracingLayer::new(SLOW_REQUEST).into_inner())
            .layer(macro_cors::cors_layer())
            .layer(CompressionLayer::new().gzip(true)),
    )
}

fn mount_at_root_and_prefix(inner: Router) -> Router {
    Router::new()
        .merge(inner.clone())
        .nest(GATEWAY_PATH_PREFIX, inner)
}
