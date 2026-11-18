//! The factory's agent definition: the Agents it may run, each a harness, a
//! model and its instructions, and for each Role the Agents that may fill
//! it, in order. The first is the Role's default; each later one takes a
//! Step the one before it could not start for lack of quota.

mod standing;
#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use bot_id::BotId;
use factory_records::domain::models::{AgentName, Role};
use factory_records::domain::standing_acts::{StandingAct, StandingRoleName};
use serde::{Deserialize, Serialize};

pub use self::standing::{StandingCheck, StandingRole};
use self::standing::StandingRoleFields;

use crate::domain::built_in_flows::built_in_roles;
use crate::domain::flow::FlowError;

/// The Role the single reviewer filled before the code Flow had a review
/// panel. Its Agents fill each Role of [`REVIEWER_ROLES`] a definition does
/// not list, so a definition written for one reviewer keeps working.
pub const REVIEWER: &str = "reviewer";

/// The Roles of the code Flow's review panel: one reviewer looks at the
/// change's correctness, one at whether it does all the task asks and no
/// more, and one at its tests.
pub const REVIEWER_ROLES: [&str; 3] = ["correctness-reviewer", "scope-reviewer", "tests-reviewer"];

/// The Role of the agent that checks a change does what its Run's request
/// asks, before and after its merge: never the writer's. A definition that
/// does not list it has [`REVIEWER`]'s Agents fill it, so a definition
/// written before it keeps working.
pub const VERIFIER: &str = "verifier";

/// Whether `role` is one of the review panel's, or the verifier, which
/// [`REVIEWER`]'s Agents fill by default.
fn defaults_to_reviewer(role: &Role) -> bool {
    REVIEWER_ROLES.contains(&role.as_str()) || role.as_str() == VERIFIER
}

/// An Agent: whose sessions it runs in, on which model, told what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Agent {
    /// The Macro agent whose sessions do the work: its harness runs them.
    /// The Run's owner must be allowed to use it.
    pub bot: BotId,
    /// The model the sessions use; the agent's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What every session of it is told before the Step's prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// The Agents, and the ones each Role takes, in order. It is written as it
/// is read, so a Run keeps its Factory Definition's and reads them back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "AgentDefinitionFields")]
pub struct AgentDefinition {
    agents: BTreeMap<AgentName, Agent>,
    roles: BTreeMap<Role, Vec<AgentName>>,
    /// The Standing Roles, which say which Agents' bots may do which acts
    /// for a Run's owner; none when the definition has no `standing`
    /// section, so any agent of the owner does every act.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    standing: Option<BTreeMap<StandingRoleName, StandingRole>>,
}

/// An [`AgentDefinition`] as written, before its checks.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentDefinitionFields {
    agents: BTreeMap<AgentName, Agent>,
    roles: BTreeMap<Role, Vec<AgentName>>,
    #[serde(default)]
    standing: Option<BTreeMap<StandingRoleName, StandingRoleFields>>,
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
    /// A Flow names a review panel's Role, or the verifier, and the
    /// definition fills neither it nor `reviewer`, whose Agents would fill
    /// it.
    #[error(
        "a flow names role {0}: the agent definition must fill it, or `reviewer`, \
         whose agents fill every reviewer role and the verifier when it does not list them"
    )]
    UnfilledReviewer(Role),
    /// A built-in Flow does not read.
    #[error(transparent)]
    Flow(#[from] FlowError),
    /// A Standing Role allows no act.
    #[error("standing role {0} allows no act")]
    StandingNoAct(StandingRoleName),
    /// A Standing Role names an act the factory does not have.
    #[error(
        "standing role {role} names act {act:?}, which is not one of \
         start, cancel, link, answer_plan, answer_blockers"
    )]
    StandingUnknownAct {
        /// The Standing Role.
        role: StandingRoleName,
        /// The act it names.
        act: String,
    },
    /// A Standing Role names a merge.
    #[error(
        "standing role {role} names act {act:?}: no standing role merges; \
         a person merges, or the merge_when_green standing rule"
    )]
    StandingMergeAct {
        /// The Standing Role.
        role: StandingRoleName,
        /// The act it names.
        act: String,
    },
    /// A Standing Role names one act twice.
    #[error("standing role {role} names act {act} twice")]
    StandingDuplicateAct {
        /// The Standing Role.
        role: StandingRoleName,
        /// The act.
        act: StandingAct,
    },
    /// No Agent fills a Standing Role.
    #[error("standing role {0} lists no agent")]
    StandingNoAgent(StandingRoleName),
    /// A Standing Role lists an Agent the definition does not have.
    #[error("standing role {role} lists agent {agent}, which the definition does not have")]
    StandingUnknownAgent {
        /// The Standing Role.
        role: StandingRoleName,
        /// The Agent it lists.
        agent: AgentName,
    },
    /// A Standing Role lists one Agent twice.
    #[error("standing role {role} lists agent {agent} twice")]
    StandingDuplicateAgent {
        /// The Standing Role.
        role: StandingRoleName,
        /// The Agent.
        agent: AgentName,
    },
    /// `FACTORY_AGENTS` has a `standing` section, which only a Factory
    /// Definition's Runs keep and check.
    #[error(
        "FACTORY_AGENTS has a standing section: standing roles are read only \
         from a factory definition's agents.json"
    )]
    StandingOutsideDefinition,
    /// Only the retired `FACTORY_AGENT_ID` is set.
    #[error(
        "FACTORY_AGENT_ID is no longer read: set FACTORY_AGENTS to the agent definition \
         (services/factory_service/AGENTS.md)"
    )]
    RetiredAgentId,
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

        let standing = match fields.standing {
            None => None,
            Some(written) => {
                let mut standing = BTreeMap::new();
                for (name, role) in written {
                    let role = role.check(&name, |agent| fields.agents.contains_key(agent))?;
                    standing.insert(name, role);
                }
                Some(standing)
            }
        };

        Ok(Self {
            agents: fields.agents,
            roles: fields.roles,
            standing,
        })
    }
}

impl AgentDefinition {
    /// The Agents that fill `role`, in order: the first is its default.
    /// A review panel's Role, or the verifier, the definition does not
    /// list takes [`REVIEWER`]'s. Empty for a Role the definition does not
    /// fill.
    pub fn for_role(&self, role: &Role) -> &[AgentName] {
        if let Some(listed) = self.roles.get(role) {
            return listed;
        }
        if defaults_to_reviewer(role) {
            return self
                .roles
                .iter()
                .find(|(listed, _)| listed.as_str() == REVIEWER)
                .map_or(&[], |(_, agents)| agents.as_slice());
        }
        &[]
    }

    /// The Agent named `name`, if the definition has it.
    pub fn agent(&self, name: &AgentName) -> Option<&Agent> {
        self.agents.get(name)
    }

    /// Whether the definition has a `standing` section, so an agent acting
    /// for a Run's owner does only the acts its bot's Standing Roles allow.
    pub fn has_standing_roles(&self) -> bool {
        self.standing.is_some()
    }

    /// Whether the bot `bot` of the calling agent session may do `act` for
    /// a Run's owner, by the definition's Standing Roles: the first
    /// Standing Role, by name, that allows the act and lists an Agent of
    /// that bot fills it. A session with no bot of the owner's (`None`)
    /// fills none.
    pub fn standing_check(&self, act: StandingAct, bot: Option<BotId>) -> StandingCheck {
        let Some(standing) = &self.standing else {
            return StandingCheck::Open;
        };

        let mut allowing = Vec::new();
        for (name, role) in standing {
            if !role.acts.contains(&act) {
                continue;
            }
            let filled_by = role
                .agents
                .iter()
                .find(|agent| {
                    self.agents
                        .get(*agent)
                        .is_some_and(|known| Some(known.bot) == bot)
                });
            if let Some(agent) = filled_by {
                return StandingCheck::Filled {
                    role: name.clone(),
                    agent: agent.clone(),
                };
            }
            allowing.push(name.clone());
        }
        StandingCheck::Refused { allowing }
    }

    /// The definition the service runs its agent Steps with: `agents`, from
    /// `FACTORY_AGENTS`, once it fills every Role of the built-in Flows.
    /// None when it is unset, so the Steps run on the stand-in, unless the
    /// retired `FACTORY_AGENT_ID` (`retired_agent_id`) is set: that is
    /// refused, rather than silently doing no work.
    pub fn configured(
        agents: Option<Self>,
        retired_agent_id: Option<&str>,
    ) -> Result<Option<Self>, AgentDefinitionError> {
        match (agents, retired_agent_id) {
            (Some(agents), _) => {
                if agents.standing.is_some() {
                    return Err(AgentDefinitionError::StandingOutsideDefinition);
                }
                agents.fills_built_in_flows()?;
                Ok(Some(agents))
            }
            (None, Some(_)) => Err(AgentDefinitionError::RetiredAgentId),
            (None, None) => Ok(None),
        }
    }

    /// Fails unless every Role the built-in Flows name has its Agents: the
    /// service checks it before it serves a Run.
    /// `reviewer` stays required beside them: a Run started before the
    /// `code` Flow had a review panel reviews with it until it ends.
    pub fn fills_built_in_flows(&self) -> Result<(), AgentDefinitionError> {
        self.fills(&built_in_roles()?)?;
        let reviewer = Role::try_from(REVIEWER.to_string())
            .map_err(|error| FlowError::Unreadable(error.to_string()))?;
        self.fills([&reviewer])
    }

    /// Fails unless every Role in `roles` has its Agents.
    pub fn fills<'a>(
        &self,
        roles: impl IntoIterator<Item = &'a Role>,
    ) -> Result<(), AgentDefinitionError> {
        for role in roles {
            if !self.for_role(role).is_empty() {
                continue;
            }
            if defaults_to_reviewer(role) {
                return Err(AgentDefinitionError::UnfilledReviewer(role.clone()));
            }
            return Err(AgentDefinitionError::UnfilledRole(role.clone()));
        }
        Ok(())
    }
}
