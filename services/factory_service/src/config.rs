//! Startup configuration.

use database_env_vars::DatabaseUrl;
use factory_engine::domain::agents::AgentDefinition;
use macro_auth::InternalApiKey;
use macro_env::Environment;
use macro_env_var::env_vars;

env_vars! {
    /// The Temporal frontend's gRPC address, as a URL (`http://temporal:7233`).
    pub struct TemporalAddress;
}

/// Settings supplied by the deployment.
#[derive(macro_config::MacroConfig)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub struct Config {
    /// The port the Run API listens on.
    #[macro_config_default(8080)]
    pub port: u16,
    /// The environment the service runs in.
    #[macro_config_default(Environment::new_or_prod())]
    pub environment: Environment,
    /// MacroDB, which keeps the Runs and their Steps.
    pub database_url: DatabaseUrl,
    /// The key internal callers present to the Run API.
    pub internal_api_key: InternalApiKey,
    /// The Temporal server the Runs' workflows run on.
    pub temporal_address: TemporalAddress,
    /// The key document storage's internal routes take, to read the tasks
    /// Runs work on.
    pub document_storage_service_auth_key: String,
    /// The key the authentication service's internal routes take, to tell
    /// which accounts exist.
    pub authentication_service_secret_key: String,
    /// The Agents that do the agent Steps, as each Run's owner, and the ones
    /// each Role takes, in order, as JSON (`FACTORY_AGENTS`). Unset, the
    /// agent Steps run on a local stand-in that does no work.
    pub factory_agents: Option<AgentDefinition>,
}
