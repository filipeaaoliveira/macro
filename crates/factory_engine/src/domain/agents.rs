//! The factory's agent definition: the Agents it may run, each a harness, a
//! model and its instructions, and for each Role the Agents that may fill
//! it, in order. The first is the Role's default; each later one takes a
//! Step the one before it could not start for lack of quota.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use bot_id::BotId;
use factory_records::domain::models::{AgentName, Role};
use serde::Deserialize;

/// An Agent: whose sessions it runs in, on which model, told what.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// The Macro agent whose sessions do the work: its harness runs them.
    /// The Run's owner must be allowed to use it.
    pub bot: BotId,
    /// The model the sessions use; the agent's default when absent.
    #[serde(default)]
    pub model: Option<String>,
    /// What every session of it is told before the Step's prompt.
    #[serde(default)]
    pub instructions: Option<String>,
}

/// The Agents, and the ones each Role takes, in order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "AgentDefinitionFields")]
pub struct AgentDefinition {
    agents: BTreeMap<AgentName, Agent>,
    roles: BTreeMap<Role, Vec<AgentName>>,
}

/// An [`AgentDefinition`] as written, before its checks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentDefinitionFields {
    agents: BTreeMap<AgentName, Agent>,
    roles: BTreeMap<Role, Vec<AgentName>>,
}

/// What is wrong with an agent definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentDefinitionError {
    /// A Role lists no Agent.
    #[error("role {0} lists no agent")]
    NoAgent(Role),
    /// A Role lists an Agent the definition does not have.
    #[error("role {role} lists agent {agent}, which the definition does not have")]
    UnknownAgent {
        /// The Role.
        role: Role,
        /// The Agent it lists.
        agent: AgentName,
    },
    /// A Role lists one Agent twice.
    #[error("role {role} lists agent {agent} twice")]
    DuplicateAgent {
        /// The Role.
        role: Role,
        /// The Agent.
        agent: AgentName,
    },
    /// A Flow names a Role the definition does not fill.
    #[error("a flow names role {0}, which the agent definition does not fill")]
    UnfilledRole(Role),
}

impl TryFrom<AgentDefinitionFields> for AgentDefinition {
    type Error = AgentDefinitionError;

    fn try_from(fields: AgentDefinitionFields) -> Result<Self, Self::Error> {
        for (role, listed) in &fields.roles {
            if listed.is_empty() {
                return Err(AgentDefinitionError::NoAgent(role.clone()));
            }

            for (place, agent) in listed.iter().enumerate() {
                if !fields.agents.contains_key(agent) {
                    return Err(AgentDefinitionError::UnknownAgent {
                        role: role.clone(),
                        agent: agent.clone(),
                    });
                }
                if listed[..place].contains(agent) {
                    return Err(AgentDefinitionError::DuplicateAgent {
                        role: role.clone(),
                        agent: agent.clone(),
                    });
                }
            }
        }

        Ok(Self {
            agents: fields.agents,
            roles: fields.roles,
        })
    }
}

impl AgentDefinition {
    /// The Agents that fill `role`, in order: the first is its default.
    /// Empty for a Role the definition does not fill.
    pub fn for_role(&self, role: &Role) -> &[AgentName] {
        self.roles.get(role).map_or(&[], Vec::as_slice)
    }

    /// The Agent named `name`, if the definition has it.
    pub fn agent(&self, name: &AgentName) -> Option<&Agent> {
        self.agents.get(name)
    }

    /// Fails unless every Role in `roles` has its Agents, as the Roles the
    /// built-in Flows name must.
    pub fn fills<'a>(
        &self,
        roles: impl IntoIterator<Item = &'a Role>,
    ) -> Result<(), AgentDefinitionError> {
        for role in roles {
            if !self.roles.contains_key(role) {
                return Err(AgentDefinitionError::UnfilledRole(role.clone()));
            }
        }
        Ok(())
    }
}
