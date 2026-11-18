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
use uuid::Uuid;

use super::{
    AutomationRequest, AutomationsSettings, CreatedAutomation, Firing, FiringResult,
    MAX_WEBHOOK_BODY_BYTES, RoutineFiring, TEAM_AUTOMATIONS_LISTED, WEBHOOK_TOLERANCE_SECS,
    WebhookFiring, run_request,
};
use crate::domain::models::FactoryError;
use crate::domain::ports::{
    AutomationService, AutomationTriggerError, AutomationTriggers, FactoryService, RunQueue,
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
pub struct AutomationServiceImpl<R, F, T, Q> {
    repo: R,
    runs: Arc<F>,
    triggers: T,
    queue: Q,
}

impl<R, F, T, Q> AutomationServiceImpl<R, F, T, Q>
where
    R: AutomationRepository + RunRepository,
    F: FactoryService,
    T: AutomationTriggers,
    Q: RunQueue,
{
    /// Create the Automation service over the records, the Run API (shared
    /// with its router), the routines that fire Automations, and the queue
    /// their Runs join.
    pub fn new(repo: R, runs: Arc<F>, triggers: T, queue: Q) -> Self {
        Self {
            repo,
            runs,
            triggers,
            queue,
        }
    }

    /// The Automation `id` of `actor`'s team, when `actor` may change it:
    /// its creator, or an admin of its team. Another team's reads as
    /// missing.
    async fn managed(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: Uuid,
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
        if automation.created_by != *actor && !team.admin {
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

        let end = self.start(automation, &firing).await?;
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
    ) -> Result<FiringEnd, FactoryError> {
        if !automation.enabled {
            return Ok(FiringEnd::Skipped(FiringSkip::Disabled));
        }
        if !self
            .queue
            .admits(&automation.created_by)
            .await
            .map_err(FactoryError::Internal)?
        {
            return Ok(FiringEnd::Skipped(FiringSkip::QueueFull));
        }
        // Never a second Run on a task while one runs there, whoever
        // started it. Two firings that race past this check are held to
        // one by the Runs' index (`TaskBusy` below).
        if let Some(task_id) = firing.task_id
            && self
                .repo
                .running_run_on_task(task_id)
                .await
                .map_err(internal)?
                .is_some()
        {
            return Ok(FiringEnd::Skipped(FiringSkip::TaskBusy));
        }

        let request = run_request(automation, firing);
        match self
            .runs
            .create_automation_run(automation, request, firing.task_id)
            .await
        {
            Ok(detail) => Ok(FiringEnd::Started(detail.run.id)),
            Err(FactoryError::TaskBusy) => Ok(FiringEnd::Skipped(FiringSkip::TaskBusy)),
            Err(
                error @ (FactoryError::Upstream(_)
                | FactoryError::Internal(_)
                | FactoryError::TriggerUnavailable),
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

impl<R, F, T, Q> AutomationService for AutomationServiceImpl<R, F, T, Q>
where
    R: AutomationRepository + RunRepository,
    F: FactoryService,
    T: AutomationTriggers,
    Q: RunQueue,
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
        id: Uuid,
        enabled: bool,
    ) -> Result<Automation, FactoryError> {
        let mut automation = self.managed(actor, id).await?;
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
        let automation = self.managed(actor, id).await?;
        if let Some(routine_id) = automation.routine_id {
            self.triggers
                .delete(&automation.created_by, routine_id)
                .await?;
        }
        self.repo.delete_automation(id).await.map_err(internal)
    }

    #[tracing::instrument(skip(self, actor, firing))]
    async fn fire_from_routine(
        &self,
        actor: &MacroUserIdStr<'static>,
        routine_id: Uuid,
        firing: RoutineFiring,
    ) -> Result<FiringResult, FactoryError> {
        // The routine is its creator's: a run of it acts as them.
        let automation = self
            .repo
            .automation_of_routine(routine_id)
            .await
            .map_err(internal)?
            .filter(|automation| automation.created_by == *actor)
            .ok_or(FactoryError::NotFound)?;
        let firing = firing.firing(&automation)?;
        self.fire(&automation, firing).await
    }

    #[tracing::instrument(skip(self, timestamp, signature, body))]
    async fn fire_webhook(
        &self,
        id: Uuid,
        timestamp: &str,
        signature: &str,
        body: &[u8],
    ) -> Result<FiringResult, FactoryError> {
        let automation = self
            .repo
            .automation(id)
            .await
            .map_err(internal)?
            .filter(|automation| automation.trigger == AutomationTrigger::Webhook)
            .ok_or(FactoryError::NotFound)?;
        let secret = self
            .repo
            .webhook_secret(id)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NotFound)?;

        // The signature covers the timestamp and the body. A call is taken
        // only within the tolerance of now, and its signature keys the
        // firing: a replay inside the tolerance finds it claimed, and one
        // past it is refused here.
        let fresh = timestamp.parse::<i64>().is_ok_and(|seconds| {
            (Utc::now().timestamp() - seconds).abs() <= WEBHOOK_TOLERANCE_SECS
        });
        if !fresh
            || body.len() > MAX_WEBHOOK_BODY_BYTES
            || !webhook_signature::verify(&secret, timestamp, body, signature)
        {
            return Err(FactoryError::BadSignature);
        }

        let call: WebhookFiring = if body.iter().all(u8::is_ascii_whitespace) {
            WebhookFiring::default()
        } else {
            serde_json::from_slice(body)
                .map_err(|_| FactoryError::InvalidFiring(super::FiringError::Body))?
        };
        let firing = Firing {
            key: signature.to_string(),
            task_id: call.task_id,
            cause: "a call to its webhook".to_string(),
        };
        let result = self.fire(&automation, firing).await?;
        if result.repeated {
            return Err(FactoryError::FiringRepeated);
        }
        Ok(result)
    }
}
