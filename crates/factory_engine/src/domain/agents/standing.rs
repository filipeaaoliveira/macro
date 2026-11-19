//! Standing Roles: the `standing` section of a Factory Definition's
//! `agents.json`. Each Standing Role names the acts it allows (start a Run,
//! cancel it, link it to a task, answer its plan or Blockers Gate) and the
//! Agents whose bots fill it. An agent acting for a Run's owner from its
//! code does an act only when its bot fills a Standing Role that allows
//! it. The acts are fixed: none merges, and a Definition cannot add one.
//! With no `standing` section, any agent of the owner does every act, as
//! before Standing Roles.

use factory_records::domain::models::AgentName;
use factory_records::domain::standing_acts::{StandingAct, StandingRoleName};
use serde::{Deserialize, Serialize};

use super::AgentDefinitionError;

/// A Standing Role: what it allows, and the Agents that fill it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StandingRole {
    /// What the Standing Role is for, for a person.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The acts it allows.
    pub acts: Vec<StandingAct>,
    /// The Agents whose bots fill it.
    pub agents: Vec<AgentName>,
}

/// A [`StandingRole`] as written, before its checks: its acts are read as
/// words, so one the factory does not have is named in the error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StandingRoleFields {
    #[serde(default)]
    description: String,
    acts: Vec<String>,
    agents: Vec<AgentName>,
}

/// Whether an act names a merge: no Standing Role may allow one, whatever
/// the Definition calls it.
fn names_a_merge(act: &str) -> bool {
    act.to_ascii_lowercase().contains("merge")
}

impl StandingRoleFields {
    /// The Standing Role `name`, checked: it allows at least one act, each
    /// one the factory has and none a merge, once, and at least one Agent
    /// fills it, each once and each one `known` says the Definition has.
    pub(super) fn check(
        self,
        name: &StandingRoleName,
        known: impl Fn(&AgentName) -> bool,
    ) -> Result<StandingRole, AgentDefinitionError> {
        if self.acts.is_empty() {
            return Err(AgentDefinitionError::StandingNoAct(name.clone()));
        }
        let mut acts = Vec::with_capacity(self.acts.len());
        for act in self.acts {
            if names_a_merge(&act) {
                return Err(AgentDefinitionError::StandingMergeAct {
                    role: name.clone(),
                    act,
                });
            }
            let Ok(known_act) = act.parse::<StandingAct>() else {
                return Err(AgentDefinitionError::StandingUnknownAct {
                    role: name.clone(),
                    act,
                });
            };
            if acts.contains(&known_act) {
                return Err(AgentDefinitionError::StandingDuplicateAct {
                    role: name.clone(),
                    act: known_act,
                });
            }
            acts.push(known_act);
        }

        if self.agents.is_empty() {
            return Err(AgentDefinitionError::StandingNoAgent(name.clone()));
        }
        for (place, agent) in self.agents.iter().enumerate() {
            if !known(agent) {
                return Err(AgentDefinitionError::StandingUnknownAgent {
                    role: name.clone(),
                    agent: agent.clone(),
                });
            }
            if self.agents[..place].contains(agent) {
                return Err(AgentDefinitionError::StandingDuplicateAgent {
                    role: name.clone(),
                    agent: agent.clone(),
                });
            }
        }

        Ok(StandingRole {
            description: self.description,
            acts,
            agents: self.agents,
        })
    }
}

/// Whether a bot may do an act, by a Definition's Standing Roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StandingCheck {
    /// The Definition has no `standing` section: any agent of the owner
    /// does every act.
    Open,
    /// The bot fills `role`, which allows the act, as the Agent `agent`.
    Filled {
        /// The Standing Role that allows the act.
        role: StandingRoleName,
        /// The Agent the bot is.
        agent: AgentName,
    },
    /// No Standing Role the bot fills allows the act: `allowing` names
    /// those that do, none when no Standing Role allows it.
    Refused {
        /// The Standing Roles that allow the act.
        allowing: Vec<StandingRoleName>,
    },
}
