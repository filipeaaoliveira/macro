//! Automations: a team's standing orders to start a Run of a Flow when its
//! trigger fires, on a schedule, a repository's GitHub event, a signed
//! webhook call or a Task event.
//!
//! A schedule, a repository event and a Task event are Macro routines of
//! the Automation's creator, made when the Automation is
//! ([`AutomationTriggers`](crate::domain::ports::AutomationTriggers)): the
//! routine's trigger is the scheduler's own cron or event trigger, and its
//! one code step fires the Automation (`POST /automations/fire`), so the
//! factory keeps no scheduler of its own. A webhook is fired by a call
//! signed with the Automation's secret (`POST /automations/{id}/webhook`).
//!
//! Each firing starts a Run of its Flow as the Automation's creator, with
//! their permissions, as if they had started it outside their signed-in
//! session: their Standing Rules and Control Files apply, and its request
//! names no Control File. A firing is keyed by what fired it, so a retry or
//! a replay starts nothing ([`AutomationRepository::claim_firing`]).
//!
//! [`AutomationRepository::claim_firing`]: factory_records::domain::automations::AutomationRepository::claim_firing

pub mod service;
#[cfg(test)]
mod test;

use factory_records::domain::automations::{Automation, AutomationTrigger, FiringSkip};
use factory_records::domain::models::{MAX_REQUEST_BYTES, RunId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::CodeRepository;

/// The most Automations a team's list holds: its newest.
pub const TEAM_AUTOMATIONS_LISTED: u16 = 100;

/// The most characters an Automation's name may hold (the
/// `factory_automation.name` check).
pub const MAX_NAME_CHARS: usize = 120;

/// The most bytes an Automation's request template may hold: a Run's
/// request, less room for the line that says what fired it.
pub const MAX_TEMPLATE_BYTES: usize = MAX_REQUEST_BYTES - 1_024;

/// How far a webhook call's timestamp may be from the factory's clock, in
/// seconds, either way: an older call is refused, so a replay past it is
/// refused without a record of every call ever made.
pub const WEBHOOK_TOLERANCE_SECS: i64 = 300;

/// The most bytes a webhook call's body may hold.
pub const MAX_WEBHOOK_BODY_BYTES: usize = 16 * 1_024;

/// The most names a repository event lists, of events or of actions.
const MAX_GITHUB_NAMES: usize = 32;

/// The most values a Task event's property may gain.
const MAX_PROPERTY_VALUES: usize = 100;

/// The most characters a Task event's value may hold.
const MAX_VALUE_CHARS: usize = 320;

/// A new Automation, as a person asks for it.
#[derive(Debug, Clone, PartialEq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRequest {
    /// What the team calls it, 1 to 120 characters.
    pub name: String,
    /// What starts its Runs.
    pub trigger: AutomationTrigger,
    /// The built-in Flow its Runs walk, one `GET /flows` lists.
    pub flow: String,
    /// The GitHub repository its Runs work on, as its
    /// `https://github.com/owner/name` URL: required by a Flow that works
    /// on one, and by a repository event, whose repository it is.
    #[serde(default)]
    pub repo_url: Option<String>,
    /// What its Runs are asked to do.
    pub request_template: String,
}

/// Why an Automation cannot be kept as asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AutomationRequestError {
    /// The name is blank or too long.
    #[error("an automation's name must be 1 to {MAX_NAME_CHARS} characters")]
    Name,
    /// The request template is blank or too long.
    #[error(
        "an automation's request must not be empty, and must be at most {MAX_TEMPLATE_BYTES} bytes"
    )]
    Template,
    /// The schedule's time zone is not an IANA one, or its cron does not
    /// have six or seven fields.
    #[error("a schedule needs a six-field cron and an IANA time zone, such as Europe/Lisbon")]
    Schedule,
    /// A repository event names no event, or a name GitHub does not use.
    #[error(
        "a repository event names 1 to 32 GitHub events and at most 32 actions, each lowercase letters or underscores"
    )]
    RepositoryEvent,
    /// A repository event's Automation names no repository, or its Flow
    /// works on none.
    #[error("a repository event needs a flow that works on a repository, and its repoUrl")]
    RepositoryEventWithoutRepository,
    /// A Task event's values are too many or too long.
    #[error("a task event's property gains at most 100 values, each 1 to 320 characters")]
    TaskEvent,
}

/// GitHub's event and action names are snake_case words.
fn github_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_lowercase() || c == '_')
}

impl AutomationRequest {
    /// Check what the Automation asks for on its own: the factory checks
    /// its Flow and repository as it would a Run's, and the scheduler its
    /// schedule and events, once the Automation is kept.
    pub fn check(&self) -> Result<(), AutomationRequestError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(AutomationRequestError::Name);
        }
        if self.request_template.trim().is_empty()
            || self.request_template.len() > MAX_TEMPLATE_BYTES
        {
            return Err(AutomationRequestError::Template);
        }
        match &self.trigger {
            AutomationTrigger::Schedule { cron, timezone } => {
                let fields = cron.split_whitespace().count();
                if !(6..=7).contains(&fields) || timezone.parse::<chrono_tz::Tz>().is_err() {
                    return Err(AutomationRequestError::Schedule);
                }
            }
            AutomationTrigger::RepositoryEvent { events, actions } => {
                let named = !events.is_empty()
                    && events.len() <= MAX_GITHUB_NAMES
                    && actions.len() <= MAX_GITHUB_NAMES
                    && events.iter().chain(actions).all(|name| github_name(name));
                if !named {
                    return Err(AutomationRequestError::RepositoryEvent);
                }
                if self.repo_url.is_none() {
                    return Err(AutomationRequestError::RepositoryEventWithoutRepository);
                }
            }
            AutomationTrigger::TaskEvent { values, .. } => {
                let valid =
                    |value: &String| !value.is_empty() && value.chars().count() <= MAX_VALUE_CHARS;
                if values.len() > MAX_PROPERTY_VALUES || !values.iter().all(valid) {
                    return Err(AutomationRequestError::TaskEvent);
                }
            }
            AutomationTrigger::Webhook => {}
        }
        Ok(())
    }
}

/// What fired an Automation, from its routine's run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutineFiring {
    /// The routine run's id: a retried run fires once.
    pub firing: String,
    /// The task a Task event is about.
    #[serde(default)]
    pub task_id: Option<Uuid>,
    /// The GitHub delivery a repository event is.
    #[serde(default)]
    pub github: Option<GithubFiring>,
}

/// The GitHub delivery that fired a repository event's Automation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubFiring {
    /// Its event, such as `pull_request`.
    pub event: String,
    /// Its payload's action, such as `opened`.
    #[serde(default)]
    pub action: Option<String>,
}

/// What a webhook call may carry, as its JSON body; an empty body carries
/// nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebhookFiring {
    /// The task the Run works on, which the Automation's creator must see.
    #[serde(default)]
    pub task_id: Option<Uuid>,
}

/// One firing of an Automation, as the factory starts its Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Firing {
    /// What keys it, so a retry or a replay starts nothing: the routine
    /// run's id, or a webhook call's signature.
    pub key: String,
    /// The task its Run works on.
    pub task_id: Option<Uuid>,
    /// What fired it, in a line its Run's request ends with.
    pub cause: String,
}

/// Why a firing is refused before anything is recorded.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FiringError {
    /// The routine run's id is blank or too long.
    #[error("a firing is keyed by its routine run's id, 1 to 200 bytes")]
    Key,
    /// A Task event's firing names no task.
    #[error("a task event's firing names its task")]
    NoTask,
    /// A repository event's firing names an event GitHub does not use.
    #[error("a repository event's firing names its GitHub event")]
    GithubEvent,
    /// A webhook call's body is not the JSON a call may carry.
    #[error("a webhook call's body is empty, or a JSON object with an optional taskId")]
    Body,
}

impl RoutineFiring {
    /// The firing of `automation` this routine run is: a Task event's names
    /// its task, and a repository event's its GitHub event. Anything else
    /// is left out: the Automation, not the routine run, says what the Run
    /// does.
    pub fn firing(self, automation: &Automation) -> Result<Firing, FiringError> {
        if self.firing.is_empty() || self.firing.len() > 200 {
            return Err(FiringError::Key);
        }
        let (task_id, cause) = match &automation.trigger {
            AutomationTrigger::TaskEvent { .. } => {
                let task_id = self.task_id.ok_or(FiringError::NoTask)?;
                (Some(task_id), format!("a change to the task {task_id}"))
            }
            AutomationTrigger::RepositoryEvent { .. } => {
                let github = self.github.ok_or(FiringError::GithubEvent)?;
                let action = github.action.unwrap_or_default();
                if !github_name(&github.event) || !(action.is_empty() || github_name(&action)) {
                    return Err(FiringError::GithubEvent);
                }
                let repository = automation
                    .repo_url
                    .as_deref()
                    .and_then(CodeRepository::parse)
                    .map(|repository| format!("{}/{}", repository.owner(), repository.name()))
                    .unwrap_or_default();
                let event = match action.as_str() {
                    "" => github.event,
                    action => format!("{} {action}", github.event),
                };
                (None, format!("the GitHub event {event} on {repository}"))
            }
            AutomationTrigger::Schedule { cron, timezone } => {
                (None, format!("its schedule ({cron}, {timezone})"))
            }
            AutomationTrigger::Webhook => (None, "a call to its webhook".to_string()),
        };
        Ok(Firing {
            key: self.firing,
            task_id,
            cause,
        })
    }
}

/// The request of the Run `automation` starts when `firing` fires it: its
/// template, then a line saying what fired it.
pub fn run_request(automation: &Automation, firing: &Firing) -> String {
    format!(
        "{}\n\nStarted by the automation \"{}\" on {}.",
        automation.request_template.trim_end(),
        automation.name,
        firing.cause,
    )
}

/// A new Automation, and the secret its webhook's calls are signed with:
/// shown once, when it is made.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreatedAutomation {
    /// The Automation.
    pub automation: Automation,
    /// For a webhook, the secret its calls are signed with: the factory
    /// never shows it again.
    pub webhook_secret: Option<String>,
}

/// A team's Automations, for one of its members.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AutomationsSettings {
    /// The team's Automations, newest first, at most
    /// [`TEAM_AUTOMATIONS_LISTED`].
    pub automations: Vec<Automation>,
    /// The team; none when the person is on no team, and so has none.
    pub team_id: Option<Uuid>,
    /// Whether the person may change any of them, as a team admin; anyone
    /// may change their own.
    pub is_admin: bool,
}

/// What a firing came to.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FiringResult {
    /// The Run it started.
    pub run_id: Option<RunId>,
    /// Why it started none.
    pub skipped: Option<FiringSkip>,
    /// Whether an earlier firing with the same key came to this, so this
    /// one started nothing.
    pub repeated: bool,
}
