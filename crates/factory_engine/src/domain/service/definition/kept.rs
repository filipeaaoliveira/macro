//! What reading a team's Factory Definition on GitHub found, kept for a
//! while per person, so the screens that read it on every load do not ask
//! GitHub again each time.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use macro_user_id::user_id::MacroUserIdStr;

use super::DefinitionHead;
use crate::domain::definition::FactoryDefinition;

/// How long the commit a Definition's default branch is at, and the
/// person's access to it, is kept for the screens that show it: a change
/// merged on GitHub shows within that time. A Run's start reads it again.
const HEAD_KEPT: Duration = Duration::from_secs(10);

/// How long a Definition read at one commit is kept. A commit's files never
/// change, so this only bounds how long a person who lost access to the
/// repository still reads what they read before.
const DEFINITION_KEPT: Duration = Duration::from_secs(600);

/// The most reads of each kind kept; past it the expired ones are dropped,
/// and if none has, all of them.
const READS_KEPT: usize = 1_000;

/// Whose read of which Definition: the person's user id and the
/// repository's URL.
type HeadKey = (String, String);

/// Whose read of which Definition at which commit: the person's user id,
/// the repository's URL and the commit.
type DefinitionKey = (String, String, String);

/// The Definition heads and contents people read lately.
#[derive(Debug, Clone)]
pub(in crate::domain::service) struct DefinitionReads {
    heads: Kept<HeadKey, DefinitionHead>,
    definitions: Kept<DefinitionKey, FactoryDefinition>,
    head_kept: Duration,
    definition_kept: Duration,
}

impl Default for DefinitionReads {
    fn default() -> Self {
        Self {
            heads: Kept::default(),
            definitions: Kept::default(),
            head_kept: HEAD_KEPT,
            definition_kept: DEFINITION_KEPT,
        }
    }
}

impl DefinitionReads {
    /// Heads and Definitions kept for `kept` each.
    #[cfg(test)]
    pub(in crate::domain::service) fn kept_for(kept: Duration) -> Self {
        Self {
            head_kept: kept,
            definition_kept: kept,
            ..Self::default()
        }
    }

    /// Where `reader` last found the Definition in `repo_url`, if that was
    /// less than [`HEAD_KEPT`] ago.
    pub(super) fn head(
        &self,
        reader: &MacroUserIdStr<'static>,
        repo_url: &str,
    ) -> Option<DefinitionHead> {
        self.heads.get(&head_key(reader, repo_url), self.head_kept)
    }

    /// Keep where `reader` found the Definition in `repo_url` now.
    pub(super) fn keep_head(
        &self,
        reader: &MacroUserIdStr<'static>,
        repo_url: &str,
        head: &DefinitionHead,
    ) {
        self.heads
            .keep(head_key(reader, repo_url), head.clone(), self.head_kept);
    }

    /// The Definition `reader` read in `repo_url` at `commit`, if they read
    /// it less than [`DEFINITION_KEPT`] ago.
    pub(super) fn definition(
        &self,
        reader: &MacroUserIdStr<'static>,
        repo_url: &str,
        commit: &str,
    ) -> Option<FactoryDefinition> {
        self.definitions.get(
            &definition_key(reader, repo_url, commit),
            self.definition_kept,
        )
    }

    /// Keep the Definition `reader` read in `repo_url` at `commit`.
    pub(super) fn keep_definition(
        &self,
        reader: &MacroUserIdStr<'static>,
        repo_url: &str,
        commit: &str,
        definition: &FactoryDefinition,
    ) {
        self.definitions.keep(
            definition_key(reader, repo_url, commit),
            definition.clone(),
            self.definition_kept,
        );
    }
}

fn head_key(reader: &MacroUserIdStr<'static>, repo_url: &str) -> HeadKey {
    (reader.as_ref().to_string(), repo_url.to_string())
}

fn definition_key(reader: &MacroUserIdStr<'static>, repo_url: &str, commit: &str) -> DefinitionKey {
    (
        reader.as_ref().to_string(),
        repo_url.to_string(),
        commit.to_string(),
    )
}

/// Values kept with the time each was kept.
#[derive(Debug)]
struct Kept<K, V> {
    values: Arc<Mutex<HashMap<K, (V, Instant)>>>,
}

impl<K, V> Clone for Kept<K, V> {
    fn clone(&self) -> Self {
        Self {
            values: Arc::clone(&self.values),
        }
    }
}

impl<K, V> Default for Kept<K, V> {
    fn default() -> Self {
        Self {
            values: Arc::default(),
        }
    }
}

impl<K: Eq + Hash, V: Clone> Kept<K, V> {
    /// The value kept for `key`, if it was kept less than `kept` ago.
    fn get(&self, key: &K, kept: Duration) -> Option<V> {
        let values = self.values.lock().ok()?;
        let (value, at) = values.get(key)?;
        (at.elapsed() < kept).then(|| value.clone())
    }

    /// Keep `value` for `key`, dropping the values older than `kept` once
    /// [`READS_KEPT`] are kept.
    fn keep(&self, key: K, value: V, kept: Duration) {
        let Ok(mut values) = self.values.lock() else {
            return;
        };
        if values.len() >= READS_KEPT {
            values.retain(|_, (_, at)| at.elapsed() < kept);
        }
        if values.len() >= READS_KEPT {
            values.clear();
        }
        values.insert(key, (value, Instant::now()));
    }
}
