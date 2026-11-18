//! The Factory Definition: the files, kept in a repository a team names and
//! changed by pull requests, that say how its factory works. It holds the
//! Flows its Runs start of, the Agents that fill each Role, the rules that
//! answer Gates and the Control Files, and the Scorers.
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
//! - `scorers.json` (optional): the Scorers, each a name and its settings.
//!
//! Secrets never live in a Definition: no field takes one, and a file that
//! holds something shaped like a key or a token does not read.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use factory_records::domain::models::Role;
use factory_records::domain::standing_rules::{
    MAX_QUEUE_LIMIT, MIN_QUEUE_LIMIT, OrchestratorDefinition,
};
use serde::Deserialize;

use crate::domain::agents::{AgentDefinition, AgentDefinitionError};
use crate::domain::control_files::{ControlFiles, ControlFilesError};
use crate::domain::flow::{Flow, FlowError, StepName, StepUse};

/// The file that names the Definition's format and its Flows.
pub const MANIFEST_FILE: &str = "factory.json";

/// The file of the Agents and the Roles they fill.
pub const AGENTS_FILE: &str = "agents.json";

/// The file of the Standing Rules and the Control File patterns.
pub const RULES_FILE: &str = "rules.json";

/// The file of the Scorers.
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

/// The format of the Definition this factory reads.
pub const DEFINITION_VERSION: u32 = 1;

/// The most Flows a Definition names.
pub const MAX_FLOWS: usize = 32;

/// The most Scorers a Definition names.
pub const MAX_SCORERS: usize = 64;

/// The most bytes one file of a Definition may hold.
pub const MAX_FILE_BYTES: usize = 256 * 1024;

/// The beginnings of keys and tokens that must never be in a Definition:
/// a file holding one at the start of a word, followed by [`SECRET_TAIL`]
/// or more characters of a token, does not read. The more specific come
/// first, so the error names them.
const SECRET_PREFIXES: [&str; 15] = [
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "glpat-",
    "sk-ant-",
    "sk-proj-",
    "sk-",
    "xoxb-",
    "xoxp-",
    "xapp-",
    "AKIA",
    "AIza",
];

/// How many token characters after a prefix make it a secret.
const SECRET_TAIL: usize = 16;

/// What begins a private key in a file.
const PRIVATE_KEY: &str = "-----BEGIN";

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
    /// Its Scorers.
    pub scorers: Vec<ScorerConfig>,
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

/// `scorers.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScorersFile {
    scorers: Vec<ScorerConfig>,
}

/// A Scorer the Definition names: its name and its own settings, which the
/// Scorer reads. The factory keeps them with the Definition; it does not
/// read the settings itself.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorerConfig {
    /// Its name: lower-case letters, digits, `-` and `_`, 1 to 64.
    pub name: String,
    /// What it scores, for a person.
    #[serde(default)]
    pub description: String,
    /// Its settings, a JSON object.
    #[serde(default)]
    pub settings: serde_json::Map<String, serde_json::Value>,
}

/// What is wrong with a Factory Definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DefinitionError {
    /// A file it needs is not there.
    #[error("{0} is missing")]
    Missing(String),
    /// A file is larger than a Definition's file may be.
    #[error("{0} is over {MAX_FILE_BYTES} bytes")]
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
    /// The Queue limit is out of range.
    #[error("{RULES_FILE}: queueLimit must be from {MIN_QUEUE_LIMIT} to {MAX_QUEUE_LIMIT}")]
    QueueLimit,
    /// A Scorer's name is not one the factory takes.
    #[error("scorer name {0:?} must be 1 to 64 lower-case letters, digits, `-` or `_`")]
    ScorerName(String),
    /// `scorers.json` names a Scorer twice.
    #[error("{SCORERS_FILE} names scorer {0} twice")]
    DuplicateScorer(String),
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

/// Whether `name` is one a Run keeps as its Flow's, and a Scorer takes:
/// what `factory_run.flow` allows.
fn is_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte))
}

/// How the first secret `text` holds begins, if it holds one.
fn secret_in(text: &str) -> Option<&'static str> {
    if text.contains(PRIVATE_KEY) {
        return Some(PRIVATE_KEY);
    }
    SECRET_PREFIXES.into_iter().find(|prefix| {
        text.match_indices(prefix).any(|(at, _)| {
            // Only at the start of a word: `task-` holds no `sk-` token.
            let word_start = text[..at]
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
/// worker serves, its Agents fill every Role its Flows name, and its rules
/// and Scorers are ones the factory takes.
pub async fn read_definition<F: DefinitionFiles>(
    files: &F,
    serves: impl Fn(&StepName) -> Option<StepUse>,
) -> Result<FactoryDefinition, DefinitionReadError<F::Err>> {
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
        let path = flow_path(&name);
        let definition: serde_json::Value = required(files, &path).await?;
        let flow = Flow::parse(definition, &serves)
            .and_then(|flow| flow.check_merges().map(|()| flow))
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
        ControlFiles::parse(patterns).map_err(DefinitionError::ControlFiles)?;
    }
    let queue_limit = rules
        .standing_rules
        .and_then(|standing_rules| standing_rules.queue_limit);
    if queue_limit.is_some_and(|limit| !(MIN_QUEUE_LIMIT..=MAX_QUEUE_LIMIT).contains(&limit)) {
        return Err(DefinitionError::QueueLimit.into());
    }

    let scorers = match json::<F, ScorersFile>(files, SCORERS_FILE).await? {
        Some(file) => file.scorers,
        None => Vec::new(),
    };
    if scorers.len() > MAX_SCORERS {
        return Err(DefinitionError::ScorerCount.into());
    }
    let mut names = BTreeSet::new();
    for scorer in &scorers {
        if !is_name(&scorer.name) {
            return Err(DefinitionError::ScorerName(scorer.name.clone()).into());
        }
        if !names.insert(scorer.name.as_str()) {
            return Err(DefinitionError::DuplicateScorer(scorer.name.clone()).into());
        }
    }

    Ok(FactoryDefinition {
        description: manifest.description,
        flows,
        agents,
        rules,
        scorers,
    })
}
