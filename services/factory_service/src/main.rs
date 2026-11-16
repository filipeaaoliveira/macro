//! Composition root of the factory service: the Run API, and the Temporal
//! worker that walks each Run's Flow.

mod config;
mod health;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use config::Config;
use factory_engine::domain::built_in_steps::BuiltInSteps;
use factory_engine::domain::service::{FactoryServiceImpl, StepServiceImpl};
use factory_engine::inbound::http::{ApiDoc, FactoryRouterState, factory_router};
use factory_engine::outbound::local_agent::LocalAgent;
use factory_engine::outbound::temporal_run_workflows::TemporalRunWorkflows;
use factory_records::outbound::pg_run_repository::PgRunRepository;
use macro_auth::middleware::decode_jwt::JwtValidationArgs;
use macro_authorization::{
    InternalAuthConfig, MacroAuthJwtValidator, MacroAuthorizationServiceImpl,
    MacroAuthorizationState, NoBotAuthorizer, NoUserApiKeyAuthorizer,
};
use macro_entrypoint::{MacroEntrypoint, shutdown_signal};
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
    let repo = PgRunRepository::new(db);

    let temporal_address = url::Url::parse(config.temporal_address.as_ref())?;
    let client = Client::connect(
        ConnectionOptions::new(temporal_address)
            .identity("factory_service".to_string())
            .build(),
        ClientOptions::new(TEMPORAL_NAMESPACE).build(),
    )
    .await?;

    // No agent runtime is wired in yet, so the agent Steps run on the local
    // stand-in, which does no work.
    let steps = BuiltInSteps::new(LocalAgent);

    let runtime = Runtime::from_current_tokio(Default::default())?;
    let mut worker = factory_engine::inbound::temporal::worker(
        &runtime,
        client.clone(),
        StepServiceImpl::new(repo.clone(), steps),
    )?;
    let stop_worker = worker.shutdown_handle();

    let secretsmanager_client = secretsmanager_client::SecretsManager::new(
        aws_sdk_secretsmanager::Client::new(&macro_aws_config::get_macro_aws_config().await),
    );
    let jwt_args =
        JwtValidationArgs::new_with_secret_manager(config.environment, &secretsmanager_client)
            .await?;
    // The Run API admits only internal callers: users, bots and user API keys
    // are refused.
    let authorization_state =
        MacroAuthorizationState::new(Arc::new(MacroAuthorizationServiceImpl::new(
            MacroAuthJwtValidator::new(jwt_args),
            InternalAuthConfig {
                api_key: config.internal_api_key.to_string(),
                default_user_id: None,
            },
            NoBotAuthorizer,
            NoUserApiKeyAuthorizer,
        )));

    let service = Arc::new(FactoryServiceImpl::new(
        repo,
        TemporalRunWorkflows::new(client),
        steps,
    ));
    let api = factory_router(FactoryRouterState::new(service, authorization_state))
        .merge(health::router());
    let app = traced(mount_at_root_and_prefix(api))
        .merge(SwaggerUi::new("/docs").url("/api-doc/openapi.json", ApiDoc::openapi()));

    let listener = tokio::net::TcpListener::bind(("0.0.0.0", config.port)).await?;
    tracing::info!(port = config.port, "factory service is up");

    let shutdown = CancellationToken::new();
    tokio::spawn({
        let shutdown = shutdown.clone();
        async move {
            shutdown_signal().await;
            shutdown.cancel();
        }
    });

    // The API and the worker stop together: on a shutdown signal, or when
    // either of them stops on its own.
    let serving = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown.clone().cancelled_owned())
        .into_future();
    let working = worker.run();
    tokio::pin!(serving, working);
    tokio::select! {
        served = &mut serving => {
            stop_worker();
            let worked = (&mut working).await;
            served?;
            worked?;
        }
        worked = &mut working => {
            shutdown.cancel();
            let served = (&mut serving).await;
            worked?;
            served?;
        }
    }
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
