//! The Automation use cases: keeping a team's Automations, and starting a
//! Run each time one fires.

use std::sync::Arc;

use chrono::Utc;
use factory_records::domain::automations::{
    Automation, AutomationRepository, AutomationTrigger, FiringClaim, FiringEnd, FiringSkip,
    NewAutomation,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use rand::Rng;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    AutomationRequest, AutomationsSettings, CreatedAutomation, Firing, FiringResult,
    MAX_WEBHOOK_BODY_BYTES, RoutineFiring, TEAM_AUTOMATIONS_LISTED, WEBHOOK_TOLERANCE_SECS,
    WebhookFiring, run_request,
};
use crate::domain::models::{ActingAgent, FactoryError};
use crate::domain::ports::{
    AutomationService, AutomationTriggerError, AutomationTriggers, FactoryService,
};

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> FactoryError {
    FactoryError::Internal(rootcause::Report::new(error).into_dynamic())
}

impl From<AutomationTriggerError> for FactoryError {
    fn from(error: AutomationTriggerError) -> Self {
        match error {
            AutomationTriggerError::Refused(reason) => FactoryError::TriggerRefused(reason),
            AutomationTriggerError::Unavailable(report) => {
                tracing::warn!(error = ?report, "the scheduler is unavailable");
                FactoryError::TriggerUnavailable
            }
        }
    }
}

/// A new webhook secret: 32 bytes from the operating system's random
/// source, as hex.
fn webhook_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    format!("whsec_{}", hex::encode(bytes))
}

/// Keeps a team's Automations, makes the routines that fire them, and
/// starts their Runs through the Run API, as their creators.
pub struct AutomationServiceImpl<R, F, T> {
    repo: R,
    runs: Arc<F>,
    triggers: T,
}

impl<R, F, T> AutomationServiceImpl<R, F, T>
where
    R: AutomationRepository + RunRepository,
    F: FactoryService,
    T: AutomationTriggers,
{
    /// Create the Automation service over the records, the Run API (shared
    /// with its router, and holding each creator's Queue to its limit) and
    /// the routines that fire Automations.
    pub fn new(repo: R, runs: Arc<F>, triggers: T) -> Self {
        Self {
            repo,
            runs,
            triggers,
        }
    }

    /// The Automation `id` of `actor`'s team, when `actor` may change it
    /// as `change` does: its creator for anything, an admin of its team only
    /// to stop it (switch it off, or delete it), since switching one on has
    /// it start Runs as its creator. Another team's reads as missing.
    async fn managed(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: Uuid,
        change: Change,
    ) -> Result<Automation, FactoryError> {
        let team = <R as RunRepository>::team_of(&self.repo, actor)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NoTeam)?;
        let automation = self
            .repo
            .automation(id)
            .await
            .map_err(internal)?
            .filter(|automation| automation.team_id == team.team_id)
            .ok_or(FactoryError::NotFound)?;
        let allowed = automation.created_by == *actor || (team.admin && change == Change::Stop);
        if !allowed {
            return Err(FactoryError::NotTheAutomationOwner);
        }
        Ok(automation)
    }

    /// Start the Run `automation` starts when `firing` fires it, unless a
    /// firing with its key did, it is switched off, the queue is full or a
    /// Run already runs on its task. A refusal the Run API gives its
    /// creator is recorded as `refused`; an outage leaves the firing
    /// claimed, for a retry once the claim is stale.
    async fn fire(
        &self,
        automation: &Automation,
        firing: Firing,
        acting: Option<ActingAgent>,
    ) -> Result<FiringResult, FactoryError> {
        let claim = self
            .repo
            .claim_firing(automation.id, &firing.key)
            .await
            .map_err(internal)?;
        match claim {
            FiringClaim::Claimed => {}
            FiringClaim::InProgress => return Err(FactoryError::FiringRepeated),
            FiringClaim::Ended(end) => return Ok(result(end, true)),
        }

        let end = self.start(automation, &firing, acting).await?;
        self.repo
            .end_firing(automation.id, &firing.key, end)
            .await
            .map_err(internal)?;
        tracing::info!(
            automation_id = %automation.id,
            trigger = %automation.trigger.kind(),
            end = ?end,
            "factory automation fired"
        );
        Ok(result(end, false))
    }

    async fn start(
        &self,
        automation: &Automation,
        firing: &Firing,
        acting: Option<ActingAgent>,
    ) -> Result<FiringEnd, FactoryError> {
        if !automation.enabled {
            return Ok(FiringEnd::Skipped(FiringSkip::Disabled));
        }
        if firing.own_branch {
            return Ok(FiringEnd::Skipped(FiringSkip::OwnBranch));
        }
        // The Automation acts as its creator only while they could make it
        // now: a person on its team.
        if <R as RunRepository>::is_agent_account(&self.repo, &automation.created_by)
            .await
            .map_err(internal)?
        {
            return Ok(FiringEnd::Skipped(FiringSkip::AgentAccount));
        }
        let team = <R as RunRepository>::team_of(&self.repo, &automation.created_by)
            .await
            .map_err(internal)?;
        if team.is_none_or(|team| team.team_id != automation.team_id) {
            return Ok(FiringEnd::Skipped(FiringSkip::CreatorLeftTeam));
        }
        let request = run_request(automation, firing);
        match self
            .runs
            .create_automation_run(automation, &firing.key, request, firing.task_id, acting)
            .await
        {
            Ok(detail) => Ok(FiringEnd::Started(detail.run.id)),
            // Never a second Run on a task while one runs there, whoever
            // started it: the Run API checks once the creator may see the
            // task, so a firing on one they cannot see is refused alike.
            Err(FactoryError::TaskBusy) => Ok(FiringEnd::Skipped(FiringSkip::TaskBusy)),
            // The Run API holds the creator's Queue to the limit of the
            // rules the Run would start under, shared with their own starts
            // and the Orchestrator's, and counts two firings at once one
            // after the other.
            Err(FactoryError::QueueFull { .. }) => Ok(FiringEnd::Skipped(FiringSkip::QueueFull)),
            Err(
                error @ (FactoryError::Upstream(_)
                | FactoryError::Internal(_)
                | FactoryError::TriggerUnavailable
                | FactoryError::FiringRepeated),
            ) => Err(error),
            Err(error) => {
                tracing::warn!(
                    automation_id = %automation.id,
                    error = %error,
                    "the factory refused an automation's run"
                );
                Ok(FiringEnd::Skipped(FiringSkip::Refused))
            }
        }
    }
}

/// Whether a run of an Automation's own routine acts as the routine
/// itself rather than as an agent: the factory makes the routine an
/// `Agent` routine, whose code step the scheduler signs for the Macro
/// routine bot (`MACRO_NEW_BOT_ID`), so that bot on the Automation's own
/// routine is its schedule or event firing it as its creator. Any other
/// bot, and code in an agent session, acts as that agent.
fn is_own_routine_run(acting: &ActingAgent) -> bool {
    matches!(acting, ActingAgent::Bot(bot) if *bot == bot_id::MACRO_NEW_BOT_ID)
}

/// What a change to an Automation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    /// It has the Automation start Runs again.
    Start,
    /// It stops the Automation starting Runs: switching it off or deleting
    /// it.
    Stop,
}

/// The key of a webhook call's firing: the SHA-256 of the timestamp and
/// body its signature covers, so the same call signed in another spelling
/// of its digest (any letter case) is the same firing.
fn webhook_firing_key(timestamp: &str, body: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(timestamp.as_bytes());
    digest.update(b".");
    digest.update(body);
    format!("sha256:{}", hex::encode(digest.finalize()))
}

/// What a firing's end answers.
fn result(end: FiringEnd, repeated: bool) -> FiringResult {
    match end {
        FiringEnd::Started(run_id) => FiringResult {
            run_id: Some(run_id),
            skipped: None,
            repeated,
        },
        FiringEnd::Skipped(skip) => FiringResult {
            run_id: None,
            skipped: Some(skip),
            repeated,
        },
    }
}

impl<R, F, T> AutomationService for AutomationServiceImpl<R, F, T>
where
    R: AutomationRepository + RunRepository,
    F: FactoryService,
    T: AutomationTriggers,
{
    #[tracing::instrument(skip_all)]
    async fn automations(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<AutomationsSettings, FactoryError> {
        let Some(team) = <R as RunRepository>::team_of(&self.repo, actor)
            .await
            .map_err(internal)?
        else {
            return Ok(AutomationsSettings {
                automations: Vec::new(),
                team_id: None,
                is_admin: false,
            });
        };
        let automations = self
            .repo
            .team_automations(team.team_id, TEAM_AUTOMATIONS_LISTED)
            .await
            .map_err(internal)?;
        Ok(AutomationsSettings {
            automations,
            team_id: Some(team.team_id),
            is_admin: team.admin,
        })
    }

    #[tracing::instrument(skip_all, fields(automation_id = tracing::field::Empty))]
    async fn create_automation(
        &self,
        actor: MacroUserIdStr<'static>,
        in_person: bool,
        request: AutomationRequest,
    ) -> Result<CreatedAutomation, FactoryError> {
        // An Automation starts Runs as its creator long after it is made,
        // so only the person, in their own session, makes one: never an
        // agent's account, nor a program acting for them.
        if !in_person
            || <R as RunRepository>::is_agent_account(&self.repo, &actor)
                .await
                .map_err(internal)?
        {
            return Err(FactoryError::NotAPerson);
        }
        request.check()?;
        let team = <R as RunRepository>::team_of(&self.repo, &actor)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NoTeam)?;
        // Its Runs must be ones its creator could start now.
        self.runs
            .check_run_order(&actor, &request.flow, request.repo_url.as_deref())
            .await?;

        let id = Uuid::now_v7();
        tracing::Span::current().record("automation_id", tracing::field::display(id));
        let secret = matches!(request.trigger, AutomationTrigger::Webhook).then(webhook_secret);
        let mut automation = self
            .repo
            .insert_automation(NewAutomation {
                id,
                team_id: team.team_id,
                created_by: &actor,
                name: request.name.trim(),
                trigger: &request.trigger,
                flow: &request.flow,
                repo_url: request.repo_url.as_deref(),
                request_template: &request.request_template,
                webhook_secret: secret.as_deref(),
            })
            .await
            .map_err(internal)?;

        // Any other trigger is the creator's routine, made once the
        // Automation is kept so that its first run finds it. A routine the
        // scheduler refuses, or that it may have made before failing, takes
        // the Automation with it: the routine fires nothing without it.
        if automation.trigger != AutomationTrigger::Webhook {
            match self.triggers.create(&automation).await {
                Ok(routine_id) => {
                    self.repo
                        .set_automation_routine(id, routine_id)
                        .await
                        .map_err(internal)?;
                    automation.routine_id = Some(routine_id);
                }
                Err(error) => {
                    self.repo.delete_automation(id).await.map_err(internal)?;
                    return Err(error.into());
                }
            }
        }

        Ok(CreatedAutomation {
            automation,
            webhook_secret: secret,
        })
    }

    #[tracing::instrument(skip(self, actor))]
    async fn set_automation_enabled(
        &self,
        actor: &MacroUserIdStr<'static>,
        in_person: bool,
        id: Uuid,
        enabled: bool,
    ) -> Result<Automation, FactoryError> {
        // Switching one on has it start Runs as its creator again: only the
        // creator, in person, as for making it.
        let change = if enabled {
            if !in_person
                || <R as RunRepository>::is_agent_account(&self.repo, actor)
                    .await
                    .map_err(internal)?
            {
                return Err(FactoryError::NotAPerson);
            }
            Change::Start
        } else {
            Change::Stop
        };
        let mut automation = self.managed(actor, id, change).await?;
        // The routine is switched first: an Automation switched on whose
        // routine stayed off would never fire.
        if let Some(routine_id) = automation.routine_id {
            self.triggers
                .set_enabled(&automation.created_by, routine_id, enabled)
                .await?;
        }
        self.repo
            .set_automation_enabled(id, enabled)
            .await
            .map_err(internal)?;
        automation.enabled = enabled;
        Ok(automation)
    }

    #[tracing::instrument(skip(self, actor))]
    async fn delete_automation(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: Uuid,
    ) -> Result<(), FactoryError> {
        let automation = self.managed(actor, id, Change::Stop).await?;
        if let Some(routine_id) = automation.routine_id {
            self.triggers
                .delete(&automation.created_by, routine_id)
                .await?;
        }
        self.repo.delete_automation(id).await.map_err(internal)
    }

    #[tracing::instrument(skip(self, actor, firing, acting))]
    async fn fire_from_routine(
        &self,
        actor: &MacroUserIdStr<'static>,
        routine_id: Uuid,
        firing: RoutineFiring,
        acting: Option<ActingAgent>,
    ) -> Result<FiringResult, FactoryError> {
        // The routine is its creator's: a run of it acts as them.
        let automation = self
            .repo
            .automation_of_routine(routine_id)
            .await
            .map_err(internal)?;
        let Some(automation) = automation else {
            // Its Automation is gone, as with its team: the routine is
            // switched off, as the caller whose routine it is, so it stops
            // firing. It is left for its owner to delete.
            if let Err(error) = self.triggers.set_enabled(actor, routine_id, false).await {
                tracing::warn!(
                    %routine_id,
                    error = %error,
                    "could not switch off the routine of a deleted automation"
                );
            }
            return Err(FactoryError::NotFound);
        };
        if automation.created_by != *actor || automation.routine_id != Some(routine_id) {
            return Err(FactoryError::NotFound);
        }
        let firing = firing.firing(&automation)?;
        let acting = acting.filter(|acting| !is_own_routine_run(acting));
        self.fire(&automation, firing, acting).await
    }

    #[tracing::instrument(skip(self, timestamp, signature, body))]
    async fn fire_webhook(
        &self,
        id: Uuid,
        timestamp: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<FiringResult, FactoryError> {
        // One read before the signature is checked, and the same answer
        // for an id no webhook has as for a bad signature, so a caller
        // without the secret learns nothing of which ids exist.
        let secret = self
            .repo
            .webhook_secret(id)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::BadSignature)?;

        // The signature covers the timestamp and the body. A call is taken
        // only within the tolerance of now, and what it signs keys the
        // firing: a replay inside the tolerance, however its digest is
        // spelled, finds it claimed, and one past it is refused here.
        let fresh = timestamp.parse::<i64>().is_ok_and(|seconds| {
            (Utc::now().timestamp() - seconds).abs() <= WEBHOOK_TOLERANCE_SECS
        });
        if !fresh
            || body.len() > MAX_WEBHOOK_BODY_BYTES
            || !webhook_signature::verify(&secret, timestamp, body, signature)
        {
            return Err(FactoryError::BadSignature);
        }
        let automation = self
            .repo
            .automation(id)
            .await
            .map_err(internal)?
            .filter(|automation| automation.trigger == AutomationTrigger::Webhook)
            .ok_or(FactoryError::BadSignature)?;

        let call: WebhookFiring = if body.iter().all(u8::is_ascii_whitespace) {
            WebhookFiring::default()
        } else {
            serde_json::from_slice(body)
                .map_err(|_| FactoryError::InvalidFiring(super::FiringError::Body))?
        };
        let firing = Firing {
            key: webhook_firing_key(timestamp, body),
            task_id: call.task_id,
            cause: "a call to its webhook".to_string(),
            own_branch: false,
        };
        // A webhook call carries no login: whoever holds the secret fires
        // it as its creator.
        let result = self.fire(&automation, firing, None).await?;
        if result.repeated {
            return Err(FactoryError::FiringRepeated);
        }
        Ok(result)
    }
}
