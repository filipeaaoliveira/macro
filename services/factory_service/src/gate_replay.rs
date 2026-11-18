//! Replays the Gate model over the Gates people answered in the factory's
//! records, and prints how often it would have agreed with them and how
//! often it would have picked wrong, at each threshold, for reading
//! replies and for standing in. The thresholds the service is given
//! (`FACTORY_GATE_REPLY_THRESHOLD`, `FACTORY_STAND_IN_THRESHOLD`) are set
//! from it.
//!
//! It reads `DATABASE_URL`, the model `FACTORY_GATE_MODEL` (Claude Haiku
//! 5.5 when unset) and how many answered Gates to read,
//! `FACTORY_GATE_REPLAY_LIMIT` (500 when unset); the model router reads its
//! provider keys from the environment, as in the service. Each call is
//! recorded against the Gate's owner, as the service's are.

use database_env_vars::DatabaseUrl;
use factory_engine::domain::gate_replies::measure::{describe, replay, runs};
use factory_engine::outbound::fast_gate_model::{DEFAULT_GATE_MODEL, FastGateModel};
use factory_records::domain::gate_replies::GateReplyRepository;
use factory_records::outbound::pg_run_repository::PgRunRepository;
use sqlx::postgres::PgPoolOptions;

/// How many answered Gates the replay reads, unless told otherwise.
const DEFAULT_LIMIT: u32 = 500;

/// What the replay reads from its environment.
#[derive(macro_config::MacroConfig)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
struct ReplayConfig {
    /// MacroDB, which keeps the Runs and their Gates.
    database_url: DatabaseUrl,
    /// The model to replay; Claude Haiku 5.5 when unset.
    #[macro_config_default(String::from(DEFAULT_GATE_MODEL))]
    factory_gate_model: String,
    /// How many answered Gates to read, oldest first.
    factory_gate_replay_limit: Option<u32>,
}

#[tokio::main]
async fn main() -> Result<(), rootcause::Report> {
    let config = macro_config::ConfigLoader::load::<ReplayConfig>()?;
    let db = PgPoolOptions::new()
        .max_connections(2)
        .connect(config.database_url.as_ref())
        .await?;
    let repo = PgRunRepository::new(db.clone());
    let gates = repo
        .answered_gates(config.factory_gate_replay_limit.unwrap_or(DEFAULT_LIMIT))
        .await?;
    let model = FastGateModel::new(config.factory_gate_model, ai_usage::pg_recorder(db));

    let report = replay(&model, &gates).await;
    println!(
        "{} answered gates read, from {} runs.\n\n{}",
        gates.len(),
        runs(&gates),
        describe(&report)
    );
    Ok(())
}
