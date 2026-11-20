//! Startup configuration.

#[cfg(test)]
mod test;

use std::num::NonZeroU8;

use database_env_vars::DatabaseUrl;
use factory_engine::domain::agents::AgentDefinition;
use factory_engine::domain::evaluation::EvaluationDefinition;
use factory_engine::outbound::fast_gate_model::DEFAULT_GATE_MODEL;
use factory_engine::outbound::model_run_judge::DEFAULT_SCORER_MODEL;
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
    /// The most review rounds a Run's review panel runs before the Run ends
    /// for a person (`FACTORY_REVIEW_ROUNDS`); the Flow's own bound, three
    /// for `code`, when unset.
    pub factory_review_rounds: Option<NonZeroU8>,
    /// The most questions each agent Step of a Run may ask the Run's owner
    /// (`FACTORY_QUESTIONS`): the next one ends the Run for a person. Two
    /// when unset; zero offers agents no question.
    pub factory_questions: Option<u8>,
    /// The model that reads a person's words on a Gate and stands in for
    /// the owner at routine Gates (`FACTORY_GATE_MODEL`), a model id Macro's
    /// model router knows; Claude Haiku 5.5 ([`DEFAULT_GATE_MODEL`]) when
    /// unset.
    #[macro_config_default(String::from(DEFAULT_GATE_MODEL))]
    pub factory_gate_model: String,
    /// How sure the Gate model must be, in percent, before a person's words
    /// answer a Gate (`FACTORY_GATE_REPLY_THRESHOLD`); the engine's default
    /// when unset.
    pub factory_gate_reply_threshold: Option<u8>,
    /// How sure the stand-in must be, in percent, before it answers a Gate
    /// (`FACTORY_STAND_IN_THRESHOLD`); the engine's default when unset.
    pub factory_stand_in_threshold: Option<u8>,
    /// How long, in seconds, the owner may override the stand-in's answer
    /// (`FACTORY_STAND_IN_WINDOW_SECS`); ten minutes when unset.
    pub factory_stand_in_window_secs: Option<u64>,
    /// The workspace's Scorers, Experiments and Benchmarks, as JSON
    /// (`FACTORY_EVALUATION`). Unset, no Run is judged, split or replayed.
    pub factory_evaluation: Option<EvaluationDefinition>,
    /// The model the Scorers judge with (`FACTORY_SCORER_MODEL`), a model id
    /// Macro's model router knows; Claude Haiku 5.5
    /// ([`DEFAULT_SCORER_MODEL`]) when unset.
    #[macro_config_default(String::from(DEFAULT_SCORER_MODEL))]
    pub factory_scorer_model: String,
    /// The retired `FACTORY_AGENT_ID`, read only to refuse it: set without
    /// `FACTORY_AGENTS`, the service does not start, and its error names
    /// `FACTORY_AGENTS`.
    pub factory_agent_id: Option<String>,
}
