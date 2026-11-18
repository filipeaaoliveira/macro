//! The Factory Definition: the files, kept in a repository a team names and
//! changed by pull requests, that say how its factory works. It holds the
//! Flows its Runs start of, the Agents that fill each Role, the rules that
//! answer Gates and the Control Files, and its Scorers, Experiments and
//! Benchmarks.
//!
//! A Run reads the Definition at the commit its repository's default branch
//! is at when the Run starts, and keeps that commit, so a change merged
//! later, or a Run's own branch, never changes the rules it runs under.
//!
//! The files, from the repository's root:
//!
//! - `factory.json`: the format's `version` and the Flows, by name.
//! - `flows/{name}.json`: each Flow, in the built-in Flows' format.
//! - `agents.json`: the Agents and the Roles they fill, in `FACTORY_AGENTS`'
//!   format. It must fill every Role its Flows name.
//! - `rules.json` (optional): the Standing Rules (`standingRules`, an
//!   Orchestrator Definition) and the Control File patterns
//!   (`controlFiles`). Left out, the factory's defaults apply.
//! - `scorers.json` (optional): the Scorers, Experiments and Benchmarks, in
//!   `FACTORY_EVALUATION`'s format, of this Definition's Flows and Agents.
//!   Left out, the team has none.
//!
//! Secrets never live in a Definition: no field takes one, and a file that
//! holds something shaped like a key or a token does not read.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use factory_records::domain::evaluation::BENCHMARK_FLOW;
use factory_records::domain::models::Role;
use factory_records::domain::standing_rules::{
    MAX_QUEUE_LIMIT, MIN_QUEUE_LIMIT, OrchestratorDefinition,
};
use serde::Deserialize;

use crate::domain::agents::{AgentDefinition, AgentDefinitionError};
use crate::domain::built_in_flows::BuiltInFlow;
use crate::domain::control_files::{ControlFiles, ControlFilesError, MAX_PATTERNS};
use crate::domain::evaluation::{EvaluationDefinition, EvaluationDefinitionError};
use crate::domain::flow::{Flow, FlowError, StepName, StepUse};

/// The file that names the Definition's format and its Flows.
pub const MANIFEST_FILE: &str = "factory.json";

/// The file of the Agents and the Roles they fill.
pub const AGENTS_FILE: &str = "agents.json";

/// The file of the Standing Rules and the Control File patterns.
pub const RULES_FILE: &str = "rules.json";

/// The file of the Scorers, Experiments and Benchmarks.
pub const SCORERS_FILE: &str = "scorers.json";

/// The Control File patterns of a Definition's own files: a Run on its
/// team's Definition repository that changes one is Tampered, so a person
/// approves every change to the factory's rules a Run makes.
pub const DEFINITION_FILE_PATTERNS: [&str; 5] = [
    MANIFEST_FILE,
    "flows/**",
    AGENTS_FILE,
    RULES_FILE,
    SCORERS_FILE,
];

/// The Steps that ran after a merge, which a Definition's Flow never runs.
const POST_MERGE_STEPS: [&str; 2] = ["deploy", "verify"];

/// The most Control File patterns a Definition's `rules.json` lists: a
/// Run on its repository adds [`DEFINITION_FILE_PATTERNS`] to them, and a
/// Run keeps at most [`MAX_PATTERNS`].
pub const MAX_DEFINITION_CONTROL_FILES: usize = MAX_PATTERNS - DEFINITION_FILE_PATTERNS.len();

/// The format of the Definition this factory reads.
pub const DEFINITION_VERSION: u32 = 1;

/// The most Flows a Definition names.
pub const MAX_FLOWS: usize = 32;

/// The most Scorers a Definition names.
pub const MAX_SCORERS: usize = 64;

/// The most KiB one file of a Definition may hold.
const MAX_FILE_KIB: usize = 256;

/// The most bytes one file of a Definition may hold.
pub const MAX_FILE_BYTES: usize = MAX_FILE_KIB * 1024;

/// The beginnings of keys and tokens that must never be in a Definition:
/// a file holding one, followed by [`SECRET_TAIL`] or more characters of a
/// token, does not read, wherever it is, glued to a URL's query or not.
/// The more specific come first, so the error names them.
const SECRET_PREFIXES: [&str; 25] = [
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "sk-ant-",
    "sk-proj-",
    "sk_live_",
    "rk_live_",
    "sk-",
    "npm_",
    "xoxa-",
    "xoxe-",
    "xoxb-",
    "xoxp-",
    "xoxr-",
    "xoxs-",
    "xapp-",
    "ya29.",
    "whsec_",
    "AKIA",
    "AIza",
    // A JSON Web Token's header, `{"`, encoded.
    "eyJ",
];

/// The prefixes that end ordinary words (`task-`): they count only at the
/// start of a word.
const WORD_START_PREFIXES: [&str; 1] = ["sk-"];

/// How many token characters after a prefix make it a secret.
const SECRET_TAIL: usize = 16;

/// What begins a private key in a file.
const PRIVATE_KEY: &str = "PRIVATE KEY";

/// What begins a key's or a certificate's block in a file.
const PEM_BEGIN: &str = "-----BEGIN";

/// What only a secret is written beside, whatever follows it, in any case:
/// a Slack webhook, an AWS secret key's name, an Azure storage key.
const SECRET_MARKERS: [&str; 3] = [
    "hooks.slack.com/services/",
    "aws_secret_access_key",
    "accountkey=",
];

/// Where a prefix begins an ordinary name, not a token: npm's settings.
const NOT_SECRETS: [&str; 1] = ["npm_config_"];

/// Whether `text` holds a URL with a user's password in it
/// (`scheme://user:password@host`).
fn credentials_in_url(text: &str) -> bool {
    text.match_indices("://").any(|(at, _)| {
        let authority = &text[at + 3..];
        let end = authority
            .find(|c: char| c.is_whitespace() || "/?#\"'".contains(c))
            .unwrap_or(authority.len());
        let Some((user_info, _)) = authority[..end].rsplit_once('@') else {
            return false;
        };
        // `redis://:password@host` names no user: a password all the same.
        user_info
            .split_once(':')
            .is_some_and(|(_, password)| !password.is_empty())
    })
}

/// The files of a Definition, read at one commit of its repository.
pub trait DefinitionFiles: Send + Sync {
    /// The error type returned when a file cannot be read.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The text of the file at `path` from the repository's root; none
    /// when there is no file there.
    fn read(&self, path: &str) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;
}

/// A Factory Definition, read and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct FactoryDefinition {
    /// What the Definition is for, for a person.
    pub description: String,
    /// Its Flows, by name.
    pub flows: BTreeMap<String, Flow>,
    /// Its Agents and the Roles they fill.
    pub agents: AgentDefinition,
    /// Its Standing Rules and Control File patterns.
    pub rules: DefinitionRules,
    /// Its Scorers, Experiments and Benchmarks.
    pub evaluation: EvaluationDefinition,
}

/// `factory.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    #[serde(default)]
    description: String,
    flows: Vec<String>,
}

/// `rules.json`: what replaces the team's settings for the Runs started
/// under the Definition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionRules {
    /// Which Gates the factory answers on its own, and how; the factory's
    /// default when left out.
    #[serde(default)]
    pub standing_rules: Option<OrchestratorDefinition>,
    /// The Control File patterns; the factory's default when left out.
    #[serde(default)]
    pub control_files: Option<Vec<String>>,
}

/// What is wrong with a Factory Definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DefinitionError {
    /// A file it needs is not there.
    #[error("{0} is missing")]
    Missing(String),
    /// A file is larger than a Definition's file may be.
    #[error("{0} is over the {MAX_FILE_KIB} KiB a definition file may hold")]
    TooLarge(String),
    /// A file is not what its name says it holds.
    #[error("{path} does not read: {error}")]
    Unreadable {
        /// The file.
        path: String,
        /// Why.
        error: String,
    },
    /// A file holds something shaped like a key or a token.
    #[error("{path} holds what looks like a secret ({kind}…): keep secrets out of the definition")]
    Secret {
        /// The file.
        path: String,
        /// How the secret begins.
        kind: &'static str,
    },
    /// `factory.json` is of a format this factory does not read.
    #[error("{MANIFEST_FILE} has version {0}; this factory reads version {DEFINITION_VERSION}")]
    Version(u32),
    /// `factory.json` names no Flow, or more than [`MAX_FLOWS`].
    #[error("{MANIFEST_FILE} must name from 1 to {MAX_FLOWS} flows")]
    FlowCount,
    /// A Flow's name is not one a Run keeps.
    #[error("flow name {0:?} must be 1 to 64 lower-case letters, digits, `-` or `_`")]
    FlowName(String),
    /// `factory.json` names a Flow twice.
    #[error("{MANIFEST_FILE} names flow {0} twice")]
    DuplicateFlow(String),
    /// A Flow does not hold together.
    #[error("{path}: {error}")]
    Flow {
        /// The Flow's file.
        path: String,
        /// What is wrong with it.
        error: FlowError,
    },
    /// The Agents do not fill a Role a Flow names.
    #[error("{AGENTS_FILE}: {0}")]
    Agents(AgentDefinitionError),
    /// The Control File patterns do not hold together.
    #[error("{RULES_FILE}: {0}")]
    ControlFiles(ControlFilesError),
    /// GitHub answers the repository the team names under another name:
    /// it was renamed or transferred.
    #[error("the definition's repository is now {0}: name it there")]
    Moved(String),
    /// The Control File patterns leave no room for the Definition's own
    /// files, which a Run on its repository adds.
    #[error(
        "{RULES_FILE}: at most {MAX_DEFINITION_CONTROL_FILES} control file patterns, as a run on the definition's repository adds its own files"
    )]
    TooManyControlFiles,
    /// The Queue limit is out of range.
    #[error("{RULES_FILE}: queueLimit must be from {MIN_QUEUE_LIMIT} to {MAX_QUEUE_LIMIT}")]
    QueueLimit,
    /// `factory.json` names a Flow the factory keeps for itself.
    #[error("{MANIFEST_FILE} names flow {0}, which only a benchmark's trials walk")]
    ReservedFlow(String),
    /// The Definition has Benchmarks, and its Agents do not fill a Role of
    /// the benchmark Flow its trials walk.
    #[error("{AGENTS_FILE}: a benchmark's trials walk the benchmark flow: {0}")]
    BenchmarkAgents(AgentDefinitionError),
    /// The Scorers, Experiments or Benchmarks name a Flow the Definition
    /// does not have, or an Agent its Agents do not.
    #[error("{SCORERS_FILE}: {0}")]
    Evaluation(EvaluationDefinitionError),
    /// `scorers.json` names more than [`MAX_SCORERS`].
    #[error("{SCORERS_FILE} names more than {MAX_SCORERS} scorers")]
    ScorerCount,
}

/// Why a Factory Definition could not be read.
#[derive(Debug, thiserror::Error)]
pub enum DefinitionReadError<E> {
    /// A file could not be read.
    #[error("the definition's files could not be read")]
    Files(#[source] E),
    /// The files do not make a Definition.
    #[error(transparent)]
    Invalid(#[from] DefinitionError),
}

/// Whether `name` is one a Run keeps as its Flow's:
/// what `factory_run.flow` allows.
fn is_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte))
}

/// How the first secret `text` holds begins, if it holds one.
fn secret_in(text: &str) -> Option<&'static str> {
    // A certificate or a public key is public: only a private key's block
    // (an RSA, EC or PGP one, whose label holds `PRIVATE KEY`) is a secret.
    let private_block = text.match_indices(PEM_BEGIN).any(|(at, _)| {
        // The block's label, up to the dashes that close it.
        let label = &text[at + PEM_BEGIN.len()..];
        let label = label.split("-----").next().unwrap_or_default();
        label.contains(PRIVATE_KEY)
    });
    if private_block {
        return Some(PRIVATE_KEY);
    }
    let lower = text.to_ascii_lowercase();
    if let Some(marker) = SECRET_MARKERS
        .into_iter()
        .find(|marker| lower.contains(marker))
    {
        return Some(marker);
    }
    if credentials_in_url(text) {
        return Some("://user:password@");
    }
    SECRET_PREFIXES.into_iter().find(|prefix| {
        text.match_indices(prefix).any(|(at, _)| {
            if NOT_SECRETS.iter().any(|name| text[at..].starts_with(name)) {
                return false;
            }
            // `task-` holds no `sk-` token: those prefixes count only at
            // the start of a word.
            let word_start = !WORD_START_PREFIXES.contains(prefix)
                || text[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| !before.is_ascii_alphanumeric());
            let tail = &text[at + prefix.len()..];
            word_start
                && tail
                    .bytes()
                    .take_while(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(byte))
                    .count()
                    >= SECRET_TAIL
        })
    })
}

/// A JSON document whose every object names each key once: serde reads a
/// key named twice as the last, which would let a file say one thing to a
/// reader and another to the factory.
struct UniqueKeys;

impl<'de> Deserialize<'de> for UniqueKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueKeysVisitor)
    }
}

struct UniqueKeysVisitor;

impl<'de> serde::de::Visitor<'de> for UniqueKeysVisitor {
    type Value = UniqueKeys;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("JSON")
    }

    fn visit_bool<E>(self, _: bool) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_i64<E>(self, _: i64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_u64<E>(self, _: u64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_f64<E>(self, _: f64) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_str<E>(self, _: &str) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_unit<E>(self) -> Result<UniqueKeys, E> {
        Ok(UniqueKeys)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<UniqueKeys, A::Error> {
        while seq.next_element::<UniqueKeys>()?.is_some() {}
        Ok(UniqueKeys)
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<UniqueKeys, A::Error> {
        let mut seen = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !seen.insert(key.clone()) {
                return Err(serde::de::Error::custom(format!(
                    "key {key:?} appears twice"
                )));
            }
            map.next_value::<UniqueKeys>()?;
        }
        Ok(UniqueKeys)
    }
}

/// How the first secret a string of `value` holds begins, keys included.
fn secret_in_value(value: &serde_json::Value) -> Option<&'static str> {
    match value {
        serde_json::Value::String(text) => secret_in(text),
        serde_json::Value::Array(items) => items.iter().find_map(secret_in_value),
        serde_json::Value::Object(fields) => fields
            .iter()
            .find_map(|(key, field)| secret_in(key).or_else(|| secret_in_value(field))),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => None,
    }
}

/// The text of `path`, checked for its size and for secrets; none when
/// there is no file there.
async fn text<F: DefinitionFiles>(
    files: &F,
    path: &str,
) -> Result<Option<String>, DefinitionReadError<F::Err>> {
    let Some(text) = files.read(path).await.map_err(DefinitionReadError::Files)? else {
        return Ok(None);
    };
    if text.len() > MAX_FILE_BYTES {
        return Err(DefinitionError::TooLarge(path.to_string()).into());
    }
    if let Some(kind) = secret_in(&text) {
        return Err(DefinitionError::Secret {
            path: path.to_string(),
            kind,
        }
        .into());
    }
    Ok(Some(text))
}

/// The file at `path`, read as JSON into `T`; none when there is no file
/// there.
async fn json<F: DefinitionFiles, T: serde::de::DeserializeOwned>(
    files: &F,
    path: &str,
) -> Result<Option<T>, DefinitionReadError<F::Err>> {
    let Some(text) = text(files, path).await? else {
        return Ok(None);
    };
    let unreadable = |error: serde_json::Error| DefinitionError::Unreadable {
        path: path.to_string(),
        error: error.to_string(),
    };
    serde_json::from_str::<UniqueKeys>(&text).map_err(unreadable)?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(unreadable)?;
    // JSON escapes (`\u005f`) hide a token from the text: its strings are
    // checked again once decoded.
    if let Some(kind) = secret_in_value(&value) {
        return Err(DefinitionError::Secret {
            path: path.to_string(),
            kind,
        }
        .into());
    }
    let read = serde_json::from_value(value).map_err(unreadable)?;
    Ok(Some(read))
}

/// The file at `path`, which must be there, read as JSON into `T`.
async fn required<F: DefinitionFiles, T: serde::de::DeserializeOwned>(
    files: &F,
    path: &str,
) -> Result<T, DefinitionReadError<F::Err>> {
    json(files, path)
        .await?
        .ok_or_else(|| DefinitionError::Missing(path.to_string()).into())
}

/// The path of the Flow named `name`.
pub fn flow_path(name: &str) -> String {
    format!("flows/{name}.json")
}

/// Read the Factory Definition `files` hold, and check it: every file
/// reads, every Flow holds together and names only Steps `serves` says a
/// worker serves, its Agents fill every Role its Flows name, its rules are
/// ones the factory takes, and its Scorers, Experiments and Benchmarks
/// name only its own Flows and Agents.
pub async fn read_definition<F: DefinitionFiles>(
    files: &F,
    serves: impl Fn(&StepName) -> Option<StepUse>,
) -> Result<FactoryDefinition, DefinitionReadError<F::Err>> {
    // A Run ends at its merge: a Definition's Flow runs none of the Steps
    // that waited for a deploy or checked the merged change.
    let serves = |step: &StepName| {
        if POST_MERGE_STEPS.contains(&step.as_str()) {
            return None;
        }
        serves(step)
    };
    let manifest: Manifest = required(files, MANIFEST_FILE).await?;
    if manifest.version != DEFINITION_VERSION {
        return Err(DefinitionError::Version(manifest.version).into());
    }
    if manifest.flows.is_empty() || manifest.flows.len() > MAX_FLOWS {
        return Err(DefinitionError::FlowCount.into());
    }

    // Each Flow the manifest names is read from its own file, and checked
    // as a built-in Flow is.
    let mut flows = BTreeMap::new();
    for name in manifest.flows {
        if !is_name(&name) {
            return Err(DefinitionError::FlowName(name).into());
        }
        if flows.contains_key(&name) {
            return Err(DefinitionError::DuplicateFlow(name).into());
        }
        // Only a Benchmark's trials walk the benchmark Flow, the factory's
        // own, which no one starts.
        if name == BENCHMARK_FLOW {
            return Err(DefinitionError::ReservedFlow(name).into());
        }
        let path = flow_path(&name);
        let definition: serde_json::Value = required(files, &path).await?;
        let flow = Flow::parse(definition, serves)
            .and_then(|flow| flow.check_merges().map(|()| flow))
            .and_then(|flow| flow.check_work_roles(serves).map(|()| flow))
            .map_err(|error| DefinitionError::Flow { path, error })?;
        flows.insert(name, flow);
    }

    // The Agents must fill every Role an entry of its Flows names.
    let agents: AgentDefinition = required(files, AGENTS_FILE).await?;
    let roles: BTreeSet<&Role> = flows
        .values()
        .flat_map(|flow| &flow.entries)
        .flat_map(|entry| entry.role.iter().chain(&entry.reviewers))
        .collect();
    agents.fills(roles).map_err(DefinitionError::Agents)?;

    let rules: DefinitionRules = json(files, RULES_FILE).await?.unwrap_or_default();
    if let Some(patterns) = &rules.control_files {
        if patterns.len() > MAX_DEFINITION_CONTROL_FILES {
            return Err(DefinitionError::TooManyControlFiles.into());
        }
        ControlFiles::parse(patterns).map_err(DefinitionError::ControlFiles)?;
    }
    let queue_limit = rules
        .standing_rules
        .and_then(|standing_rules| standing_rules.queue_limit);
    if queue_limit.is_some_and(|limit| !(MIN_QUEUE_LIMIT..=MAX_QUEUE_LIMIT).contains(&limit)) {
        return Err(DefinitionError::QueueLimit.into());
    }

    // A Definition's Scorers and Experiments judge and split its own
    // Flows' Runs, and its arms choose among its own Agents.
    let evaluation: EvaluationDefinition = json(files, SCORERS_FILE).await?.unwrap_or_default();
    if evaluation.scorers().len() > MAX_SCORERS {
        return Err(DefinitionError::ScorerCount.into());
    }
    let evaluation = evaluation
        .checked(|flow| flows.contains_key(flow), Some(&agents))
        .map_err(DefinitionError::Evaluation)?;
    // A Benchmark's trials walk the factory's benchmark Flow on the
    // Definition's Agents, which must fill its Roles.
    if !evaluation.benchmarks().is_empty() {
        let benchmark = BuiltInFlow::named(BENCHMARK_FLOW)
            .ok_or_else(|| FlowError::Unreadable(format!("no built-in flow {BENCHMARK_FLOW}")))
            .and_then(|built_in| built_in.read(&serves))
            .map_err(|error| DefinitionError::Flow {
                path: BENCHMARK_FLOW.to_string(),
                error,
            })?;
        let roles: BTreeSet<&Role> = benchmark
            .entries
            .iter()
            .flat_map(|entry| entry.role.iter().chain(&entry.reviewers))
            .collect();
        agents
            .fills(roles)
            .map_err(DefinitionError::BenchmarkAgents)?;
    }

    Ok(FactoryDefinition {
        description: manifest.description,
        flows,
        agents,
        rules,
        evaluation,
    })
}
