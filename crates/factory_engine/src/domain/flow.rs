//! A Flow: the entries a Run walks, the Step each one runs, and where each
//! Step's outcome takes the Run. Flows are data, written as JSON.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU8;

use factory_records::domain::models::{
    EntryId, InvalidName, MAX_QUESTION_BYTES, Outcome, Role, RunOutcome, checked_name,
};
use serde::{Deserialize, Serialize};

/// A fact a Flow keeps while a Run goes: a yes or no its transitions set and
/// its conditions read, such as whether a review panel has run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FactName(String);

impl TryFrom<String> for FactName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("fact", value).map(Self)
    }
}

impl From<FactName> for String {
    fn from(name: FactName) -> Self {
        name.0
    }
}

impl fmt::Display for FactName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of the work a Step does, such as `reply`. The worker that runs a
/// Flow's Steps says which names it serves.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StepName(String);

impl StepName {
    /// The name as written in the Flow.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StepName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("step", value).map(Self)
    }
}

impl From<StepName> for String {
    fn from(name: StepName) -> Self {
        name.0
    }
}

impl fmt::Display for StepName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a transition takes the Run: to an entry, or to the Run's end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// Run this entry next.
    Entry {
        /// The entry.
        entry: EntryId,
    },
    /// End the Run.
    End {
        /// How the Run ends.
        outcome: RunOutcome,
    },
}

/// A condition on one of the Run's facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// The fact it reads.
    pub fact: FactName,
    /// The value the fact must have.
    pub equals: bool,
}

/// One way out of an entry. It applies when all its conditions hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// Conditions on the Run's facts; none always holds.
    #[serde(default)]
    pub when: Vec<Condition>,
    /// Where it takes the Run.
    pub to: Target,
    /// Facts it sets.
    #[serde(default)]
    pub set: BTreeMap<FactName, bool>,
    /// Entries whose visits it counts from zero again.
    #[serde(default)]
    pub reset: Vec<EntryId>,
}

impl Transition {
    /// A transition that only moves the Run to `to`.
    pub fn to(to: Target) -> Self {
        Self {
            when: Vec::new(),
            to,
            set: BTreeMap::new(),
            reset: Vec::new(),
        }
    }
}

/// A question an entry puts to a person once its Step ends `done`. The Run
/// waits for the answer, and the answer is the outcome the Flow routes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateDefinition {
    /// The question, shown with the Step's output.
    pub question: String,
    /// The answers a person may give.
    pub answers: Vec<Outcome>,
}

/// One entry of a Flow: a Step to run, when, how often, and where each of its
/// outcomes goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The entry's id, unique in its Flow.
    pub id: EntryId,
    /// The Step it runs.
    pub step: StepName,
    /// The Role an agent fills to do the Step, when an agent does it: the
    /// factory's agent definition says which Agents fill it.
    #[serde(default)]
    pub role: Option<Role>,
    /// Conditions on the Run's facts; when one does not hold the entry is
    /// skipped and the Run goes to its `next`.
    #[serde(default)]
    pub when: Vec<Condition>,
    /// How many times the entry may run before the Run takes `on_exhausted`.
    #[serde(default)]
    pub max_visits: Option<NonZeroU8>,
    /// Where the Run goes once the entry ran `max_visits` times: the first
    /// transition whose conditions hold, else the Run ends blocked.
    #[serde(default)]
    pub on_exhausted: Vec<Transition>,
    /// Where an outcome with no transition in `on`, or a skip, goes; the
    /// entry below when absent.
    #[serde(default)]
    pub next: Option<Target>,
    /// Where each outcome goes: the first transition whose conditions hold.
    #[serde(default)]
    pub on: BTreeMap<Outcome, Vec<Transition>>,
    /// The question the entry asks once its Step ends `done`; its answer is
    /// then the outcome `on` routes.
    #[serde(default)]
    pub gate: Option<GateDefinition>,
}

/// A Flow definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    /// What the Flow does, for a person.
    #[serde(default)]
    pub description: String,
    /// Whether a Run of it works on a GitHub repository: one is started with
    /// the repository, and the others with none.
    #[serde(default)]
    pub repository: bool,
    /// The facts the Flow keeps, with their values when a Run starts.
    #[serde(default)]
    pub facts: BTreeMap<FactName, bool>,
    /// The entries, in order. A Run starts at the first.
    pub entries: Vec<Entry>,
}

/// What is wrong with a Flow definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    /// The definition is not a Flow.
    #[error("the flow does not read: {0}")]
    Unreadable(String),
    /// The Flow has no entries.
    #[error("the flow has no entries")]
    NoEntries,
    /// Two entries share an id.
    #[error("entry {0} appears twice")]
    DuplicateEntry(EntryId),
    /// A target, a reset or a cursor names an entry the Flow does not have.
    #[error("entry {from} names entry {to}, which the flow does not have")]
    UnknownEntry {
        /// The entry that names it.
        from: EntryId,
        /// The entry it names.
        to: EntryId,
    },
    /// A condition or a transition names a fact the Flow does not declare.
    #[error("entry {entry} names fact {fact}, which the flow does not declare")]
    UnknownFact {
        /// The entry that names it.
        entry: EntryId,
        /// The fact it names.
        fact: FactName,
    },
    /// A Gate has no question, a question too long to keep, or no answers.
    #[error(
        "entry {0} has a gate with no question, a question over {MAX_QUESTION_BYTES} bytes, or no answers"
    )]
    IncompleteGate(EntryId),
    /// A Gate offers one answer twice.
    #[error("entry {entry}'s gate offers {answer} twice")]
    DuplicateAnswer {
        /// The entry.
        entry: EntryId,
        /// The answer.
        answer: Outcome,
    },
    /// An entry runs a Step the worker does not serve.
    #[error("entry {entry} runs step {step}, which no worker serves")]
    UnknownStep {
        /// The entry.
        entry: EntryId,
        /// Its Step.
        step: StepName,
    },
}

impl Flow {
    /// Read a Flow from its JSON definition and check that it holds together:
    /// unique entries, and every entry, fact and Step it names known.
    pub fn parse(
        definition: serde_json::Value,
        serves: impl Fn(&StepName) -> bool,
    ) -> Result<Self, FlowError> {
        let flow: Flow = serde_json::from_value(definition)
            .map_err(|error| FlowError::Unreadable(error.to_string()))?;
        flow.check(serves)?;
        Ok(flow)
    }

    fn check(&self, serves: impl Fn(&StepName) -> bool) -> Result<(), FlowError> {
        if self.entries.is_empty() {
            return Err(FlowError::NoEntries);
        }

        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            if !ids.insert(&entry.id) {
                return Err(FlowError::DuplicateEntry(entry.id.clone()));
            }
        }

        for entry in &self.entries {
            if !serves(&entry.step) {
                return Err(FlowError::UnknownStep {
                    entry: entry.id.clone(),
                    step: entry.step.clone(),
                });
            }

            if let Some(gate) = &entry.gate {
                check_gate(&entry.id, gate)?;
            }

            let transitions = entry.on.values().flatten().chain(&entry.on_exhausted);
            let mut targets: Vec<&Target> = transitions.clone().map(|t| &t.to).collect();
            targets.extend(&entry.next);
            let mut named: Vec<&EntryId> = transitions.clone().flat_map(|t| &t.reset).collect();
            for target in targets {
                if let Target::Entry { entry } = target {
                    named.push(entry);
                }
            }
            if let Some(unknown) = named.into_iter().find(|id| !ids.contains(id)) {
                return Err(FlowError::UnknownEntry {
                    from: entry.id.clone(),
                    to: unknown.clone(),
                });
            }

            let read = transitions.clone().flat_map(|t| &t.when).map(|c| &c.fact);
            let set = transitions.flat_map(|t| t.set.keys());
            let mut facts = entry.when.iter().map(|c| &c.fact).chain(read).chain(set);
            if let Some(unknown) = facts.find(|fact| !self.facts.contains_key(fact)) {
                return Err(FlowError::UnknownFact {
                    entry: entry.id.clone(),
                    fact: unknown.clone(),
                });
            }
        }
        Ok(())
    }
}

fn check_gate(entry: &EntryId, gate: &GateDefinition) -> Result<(), FlowError> {
    let question = gate.question.trim();
    if question.is_empty() || gate.question.len() > MAX_QUESTION_BYTES || gate.answers.is_empty() {
        return Err(FlowError::IncompleteGate(entry.clone()));
    }

    let mut offered = BTreeSet::new();
    if let Some(twice) = gate.answers.iter().find(|answer| !offered.insert(*answer)) {
        return Err(FlowError::DuplicateAnswer {
            entry: entry.clone(),
            answer: twice.clone(),
        });
    }
    Ok(())
}
