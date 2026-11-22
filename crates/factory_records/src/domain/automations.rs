//! Automations: a team's standing orders to start a Run of a Flow when
//! something happens, and each time one fired.

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::RunId;

/// What starts an Automation's Runs, as a kind.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum TriggerKind {
    /// A schedule.
    Schedule,
    /// An event of the Automation's repository on GitHub.
    RepositoryEvent,
    /// A call to the Automation's webhook, signed with its secret.
    Webhook,
    /// A change to a task.
    TaskEvent,
}

/// What starts an Automation's Runs, with the kind's own settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationTrigger {
    /// A schedule, as a Macro routine's cron trigger.
    Schedule {
        /// Six fields: seconds, minutes, hours, day of month, month and
        /// weekday, such as `0 0 9 * * Mon-Fri`.
        cron: String,
        /// The IANA time zone the schedule is read in, such as
        /// `Europe/Lisbon`.
        timezone: String,
    },
    /// A delivery from Macro's GitHub App about the Automation's
    /// repository.
    #[serde(rename_all = "camelCase")]
    RepositoryEvent {
        /// GitHub event names, such as `push` or `pull_request`: at least
        /// one.
        events: Vec<String>,
        /// Payload actions, such as `opened`; any when empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        actions: Vec<String>,
    },
    /// A call to the Automation's webhook, signed with its secret.
    Webhook,
    /// A task's property gaining one of some values, such as its status
    /// becoming `In Progress` or its tags gaining one.
    #[serde(rename_all = "camelCase")]
    TaskEvent {
        /// The property definition that must change, such as the status's.
        property_id: Uuid,
        /// Select option ids or referenced entity ids, one of which the
        /// change must add; any change to the property when empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        values: Vec<String>,
    },
}

impl AutomationTrigger {
    /// Its kind.
    pub fn kind(&self) -> TriggerKind {
        match self {
            Self::Schedule { .. } => TriggerKind::Schedule,
            Self::RepositoryEvent { .. } => TriggerKind::RepositoryEvent,
            Self::Webhook => TriggerKind::Webhook,
            Self::TaskEvent { .. } => TriggerKind::TaskEvent,
        }
    }
}

/// A team's standing order to start a Run of a Flow when its trigger fires.
/// Its Runs are its creator's, as if they had started them.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    /// Its id.
    pub id: Uuid,
    /// The team it belongs to.
    pub team_id: Uuid,
    /// Who created it: the owner of the Runs it starts.
    pub created_by: MacroUserIdStr<'static>,
    /// What its team calls it.
    pub name: String,
    /// What starts its Runs.
    pub trigger: AutomationTrigger,
    /// The built-in Flow its Runs walk, by name.
    pub flow: String,
    /// The repository its Runs work on, for a Flow that works on one.
    pub repo_url: Option<String>,
    /// What its Runs are asked to do.
    pub request_template: String,
    /// Whether it starts Runs.
    pub enabled: bool,
    /// The Macro routine that fires it, for a schedule, a repository event
    /// or a Task event, once it is made.
    pub routine_id: Option<Uuid>,
    /// The scheduler's configuration revision of that routine as the factory
    /// last wrote it (made, or switched on): a run the scheduler started
    /// itself at this revision fires it as its creator. Not shown: it says
    /// nothing to a person.
    #[serde(skip)]
    pub routine_revision: Option<i64>,
    /// When it was created.
    pub created_at: DateTime<Utc>,
}

/// A new Automation to keep.
#[derive(Debug, Clone, Copy)]
pub struct NewAutomation<'a> {
    /// Its id.
    pub id: Uuid,
    /// The team it belongs to.
    pub team_id: Uuid,
    /// Who creates it.
    pub created_by: &'a MacroUserIdStr<'static>,
    /// What its team calls it.
    pub name: &'a str,
    /// What starts its Runs.
    pub trigger: &'a AutomationTrigger,
    /// The built-in Flow its Runs walk.
    pub flow: &'a str,
    /// The repository its Runs work on, if its Flow works on one.
    pub repo_url: Option<&'a str>,
    /// What its Runs are asked to do.
    pub request_template: &'a str,
    /// The secret its webhook's calls are signed with: set for, and only
    /// for, a webhook.
    pub webhook_secret: Option<&'a str>,
}

/// The Automation that started a Run, as it was then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunAutomation {
    /// The Automation's id; it may have been deleted since.
    pub id: Uuid,
    /// Its name when it started the Run.
    pub name: String,
    /// What kind of trigger started the Run.
    pub trigger: TriggerKind,
}

/// Why a firing started no Run.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::Display,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum FiringSkip {
    /// The Automation was switched off.
    Disabled,
    /// A Run already runs on the task.
    TaskBusy,
    /// The creator's queue of Runs is full.
    QueueFull,
    /// The factory refused the Run, as it would refuse its creator: the
    /// task or repository is out of their reach, or the Flow is gone.
    Refused,
    /// The repository event is about a branch a factory Run works on, so
    /// the Run's own pushes and pull requests start no other.
    OwnBranch,
    /// The Automation's creator is no longer on its team.
    CreatorLeftTeam,
    /// The Automation's creator's account is an agent's.
    AgentAccount,
}

/// What a firing came to, once it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiringEnd {
    /// It started this Run.
    Started(RunId),
    /// It started none, for this reason.
    Skipped(FiringSkip),
}

/// The Automation that starts a new Run, and the firing it starts it for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutomationStart<'a> {
    /// The Automation, as it is now.
    pub automation: &'a RunAutomation,
    /// The firing's key: a firing starts at most one Run.
    pub firing_key: &'a str,
}

/// What claiming a firing found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiringClaim {
    /// No firing had this key, or one that never ended was claimed long
    /// enough ago to be taken over: this one goes on.
    Claimed,
    /// A firing with this key ended, as it says.
    Ended(FiringEnd),
    /// A firing with this key is going on.
    InProgress,
}

/// How long a claimed firing that never ended keeps its key: past it, the
/// worker that claimed it is taken to have stopped, and the next firing
/// with the key goes on, unless a Run started under the key. Three times
/// the ten minutes a webhook call's signature is taken (its timestamp
/// within five minutes of now, either way), so a replay never takes a
/// firing over.
pub const FIRING_CLAIM_STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(30);

/// Where Automations and their firings are kept.
pub trait AutomationRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Keep a new Automation, enabled and with no routine yet.
    fn insert_automation(
        &self,
        automation: NewAutomation<'_>,
    ) -> impl Future<Output = Result<Automation, Self::Err>> + Send;

    /// Record the routine that fires the Automation `id`, at the scheduler's
    /// configuration revision `revision` the factory wrote it at.
    fn set_automation_routine(
        &self,
        id: Uuid,
        routine_id: Uuid,
        revision: i64,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The Automation `id`, if it exists.
    fn automation(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<Automation>, Self::Err>> + Send;

    /// The Automation the routine `routine_id` fires, if any.
    fn automation_of_routine(
        &self,
        routine_id: Uuid,
    ) -> impl Future<Output = Result<Option<Automation>, Self::Err>> + Send;

    /// The newest `limit` Automations of the team `team_id`, newest first.
    fn team_automations(
        &self,
        team_id: Uuid,
        limit: u16,
    ) -> impl Future<Output = Result<Vec<Automation>, Self::Err>> + Send;

    /// The secret the webhook Automation `id` is signed with, while it is
    /// a webhook Automation.
    fn webhook_secret(
        &self,
        id: Uuid,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// Switch the Automation `id` on or off, with its routine now at the
    /// configuration revision `routine_revision` the factory wrote it at,
    /// when it wrote it; false when it does not exist.
    fn set_automation_enabled(
        &self,
        id: Uuid,
        enabled: bool,
        routine_revision: Option<i64>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Remove the Automation `id` and its firings. The Runs it started keep
    /// the record of it.
    fn delete_automation(&self, id: Uuid) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Claim the firing of the Automation `automation_id` keyed `key`, at
    /// the database's time: a key is claimed once, unless its firing never
    /// ended and was claimed over [`FIRING_CLAIM_STALE_AFTER`] ago.
    fn claim_firing(
        &self,
        automation_id: Uuid,
        key: &str,
    ) -> impl Future<Output = Result<FiringClaim, Self::Err>> + Send;

    /// Record what the firing keyed `key` came to. Its first end is kept.
    fn end_firing(
        &self,
        automation_id: Uuid,
        key: &str,
        end: FiringEnd,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// A Run that works on the task document `task_id` and has not ended,
    /// whoever owns it.
    fn running_run_on_task(
        &self,
        task_id: Uuid,
    ) -> impl Future<Output = Result<Option<RunId>, Self::Err>> + Send;
}
