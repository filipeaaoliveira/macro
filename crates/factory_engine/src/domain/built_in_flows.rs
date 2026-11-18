//! The Flows people start Runs of: the definitions in `flows/`, each named
//! for its file.

#[cfg(test)]
mod test;

use std::collections::BTreeSet;
use std::sync::{LazyLock, Mutex};

use factory_records::domain::models::{Role, RunFlow, RunId};

use crate::domain::flow::{Flow, FlowError, StepName, StepUse};

/// The Runs whose kept Flow was found not to read, warned of once each.
static UNREADABLE_FLOWS: LazyLock<Mutex<BTreeSet<RunId>>> = LazyLock::new(Mutex::default);

/// A built-in Flow: the name a Run of it is started with, and its JSON
/// definition.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInFlow {
    /// The Flow's name.
    pub name: &'static str,
    definition: &'static str,
}

/// Every built-in Flow, by name.
pub const BUILT_IN_FLOWS: [BuiltInFlow; 4] = [
    BuiltInFlow {
        name: "code",
        definition: include_str!("../../flows/code.json"),
    },
    BuiltInFlow {
        name: "plan",
        definition: include_str!("../../flows/plan.json"),
    },
    BuiltInFlow {
        name: "reply",
        definition: include_str!("../../flows/reply.json"),
    },
    // Runs only as a Benchmark suite's trial: never offered to people.
    BuiltInFlow {
        name: "benchmark",
        definition: include_str!("../../flows/benchmark.json"),
    },
];

impl BuiltInFlow {
    /// The built-in Flow named `name`, if there is one.
    pub fn named(name: &str) -> Option<Self> {
        BUILT_IN_FLOWS.into_iter().find(|flow| flow.name == name)
    }

    /// Read the Flow, and check it holds together and names only Steps that
    /// `serves` says a worker serves, and how.
    pub fn read(&self, serves: impl Fn(&StepName) -> Option<StepUse>) -> Result<Flow, FlowError> {
        let definition = serde_json::from_str(self.definition)
            .map_err(|error| FlowError::Unreadable(error.to_string()))?;
        Flow::parse(definition, serves)
    }
}

/// The Roles the built-in Flows' entries name: the factory's agent
/// definition must fill each one.
pub fn built_in_roles() -> Result<BTreeSet<Role>, FlowError> {
    let mut roles = BTreeSet::new();
    for flow in BUILT_IN_FLOWS {
        let flow = flow.read(|_| Some(StepUse::Work))?;
        for entry in flow.entries {
            roles.extend(entry.role);
            roles.extend(entry.reviewers);
        }
    }
    Ok(roles)
}

/// The Flow a Run walks, from what it keeps: the Flow it kept when it
/// started, or, for a Run kept before Runs kept theirs, the built-in Flow
/// of its name. None when neither reads.
pub fn walked_flow(run_id: RunId, kept: RunFlow) -> Option<Flow> {
    match kept.definition {
        Some(definition) => serde_json::from_value(definition)
            .inspect_err(|error| {
                // Each list of Runs reads their Flows again: one warning a
                // Run, for this worker's life.
                let first = UNREADABLE_FLOWS
                    .lock()
                    .is_ok_and(|mut warned| warned.insert(run_id));
                if first {
                    tracing::warn!(%run_id, ?error, "a run's kept flow does not read");
                }
            })
            .ok(),
        None => BuiltInFlow::named(kept.name.as_deref()?)?
            .read(|_| Some(StepUse::Work))
            .ok(),
    }
}
