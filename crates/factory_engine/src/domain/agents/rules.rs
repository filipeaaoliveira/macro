//! Agent Rules: the `rules` section of a Factory Definition's `agents.json`.
//! Each rule names a Role, what it matches (a tag of the Run's task, the
//! repository the Run works on, or both) and the Agent the Role's Steps
//! start with when it matches, in place of the Role's first. The Role's own
//! Agents follow it, in their order, when it cannot start for lack of
//! quota.
//!
//! A Run's rules are matched once, as it starts, and the Run keeps the rule
//! that chose each Role's Agent, so its Steps never match again: a tag
//! changed later, or a changed Definition, does not change a Run under way.
//! Rules are read in the order the Definition lists them, and the first
//! that matches a Role chooses its Agent. An Experiment arm that chooses an
//! Agent for the same Role comes before any rule.

use factory_records::domain::evaluation::{AgentChoices, RunAgentRule};
use factory_records::domain::models::{AgentName, Role, checked_name};
use serde::{Deserialize, Serialize};

use super::AgentDefinitionError;

/// The most Agent Rules a definition holds.
pub const MAX_AGENT_RULES: usize = 64;

/// The most characters of a rule's tag.
pub const MAX_TAG_CHARS: usize = 64;

/// The prefix of a repository's URL that a rule's `owner/name` follows.
const GITHUB: &str = "https://github.com/";

/// An Agent Rule: when it matches a Run, the Run's Steps of `role` start
/// with `agent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRule {
    /// Its name, as a Run keeps the rule that chose its Agent.
    pub name: String,
    /// The Role whose Agent it chooses.
    pub role: Role,
    /// A tag the Run's task must carry, ignoring case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The repository the Run must work on, as `owner/name`, ignoring case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// The Agent the Role's Steps start with.
    pub agent: AgentName,
}

/// What a Run's rules are matched against, as it starts.
#[derive(Debug, Clone, Copy, Default)]
pub struct RuleSubject<'a> {
    /// The tags of its task, as its owner sees them; none for a Run on no
    /// task.
    pub tags: &'a [String],
    /// Its repository's `https://github.com/owner/name` URL; none for a Run
    /// on none.
    pub repository: Option<&'a str>,
}

/// Whether `repository` reads as `owner/name`.
fn is_owner_and_name(repository: &str) -> bool {
    let mut parts = repository.split('/');
    let readable = |part: Option<&str>| {
        part.is_some_and(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
        })
    };
    readable(parts.next()) && readable(parts.next()) && parts.next().is_none()
}

impl AgentRule {
    /// Fails unless the rule reads: its name is a name a Run keeps, it
    /// matches something, its tag is 1 to [`MAX_TAG_CHARS`] characters,
    /// and its repository is `owner/name`.
    pub(super) fn check(&self) -> Result<(), AgentDefinitionError> {
        checked_name("agent rule", self.name.clone())
            .map_err(|_| AgentDefinitionError::RuleName(self.name.clone()))?;
        if self.tag.is_none() && self.repository.is_none() {
            return Err(AgentDefinitionError::RuleMatchesNothing(self.name.clone()));
        }
        if let Some(tag) = &self.tag
            && !(1..=MAX_TAG_CHARS).contains(&tag.trim().chars().count())
        {
            return Err(AgentDefinitionError::RuleTag(self.name.clone()));
        }
        if let Some(repository) = &self.repository
            && !is_owner_and_name(repository)
        {
            return Err(AgentDefinitionError::RuleRepository(self.name.clone()));
        }
        Ok(())
    }

    /// Whether the rule matches `subject`: every condition it names holds.
    pub fn matches(&self, subject: RuleSubject<'_>) -> bool {
        let tagged = self.tag.as_ref().is_none_or(|tag| {
            subject
                .tags
                .iter()
                .any(|carried| carried.trim().eq_ignore_ascii_case(tag.trim()))
        });
        let on_repository = self.repository.as_ref().is_none_or(|repository| {
            subject.repository.is_some_and(|url| {
                url.strip_prefix(GITHUB)
                    .unwrap_or(url)
                    .trim_end_matches('/')
                    .eq_ignore_ascii_case(repository)
            })
        });
        tagged && on_repository
    }

    /// The rule's choice, as an arm's: its Role and its Agent.
    pub fn choice(&self) -> AgentChoices {
        [(self.role.clone(), self.agent.clone())]
            .into_iter()
            .collect()
    }
}

/// The rules of `rules` that choose an Agent for a Run matched as
/// `subject`: the first that matches each Role, in the order they are
/// listed, leaving out the Roles `taken` says something else chose for.
pub fn chosen_by_rules(
    rules: &[AgentRule],
    subject: RuleSubject<'_>,
    taken: impl Fn(&Role) -> bool,
) -> Vec<RunAgentRule> {
    let mut chosen: Vec<RunAgentRule> = Vec::new();
    for rule in rules {
        if taken(&rule.role) || chosen.iter().any(|kept| kept.role == rule.role) {
            continue;
        }
        if rule.matches(subject) {
            chosen.push(RunAgentRule {
                role: rule.role.clone(),
                rule: rule.name.clone(),
                agent: rule.agent.clone(),
            });
        }
    }
    chosen
}
