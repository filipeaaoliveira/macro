//! The Flows people start Runs of: the definitions in `flows/`, each named
//! for its file.

#[cfg(test)]
mod test;

use std::collections::BTreeSet;

use factory_records::domain::models::Role;

use crate::domain::flow::{Flow, FlowError, StepName, StepUse};

/// A built-in Flow: the name a Run of it is started with, and its JSON
/// definition.
#[derive(Debug, Clone, Copy)]
pub struct BuiltInFlow {
    /// The Flow's name.
    pub name: &'static str,
    definition: &'static str,
}

/// Every built-in Flow, by name.
pub const BUILT_IN_FLOWS: [BuiltInFlow; 3] = [
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
        roles.extend(flow.entries.into_iter().filter_map(|entry| entry.role));
    }
    Ok(roles)
}
