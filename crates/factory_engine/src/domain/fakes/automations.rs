//! Automations and their firings in memory, with the same first-write-wins
//! rules as the Postgres repository. A claim in memory is never stale.

use chrono::Utc;
use factory_records::domain::automations::{
    Automation, AutomationRepository, FiringClaim, FiringEnd, NewAutomation,
};
use factory_records::domain::models::RunId;
use uuid::Uuid;

use super::{FakeError, FakeRepo};

impl AutomationRepository for FakeRepo {
    type Err = FakeError;

    async fn insert_automation(&self, new: NewAutomation<'_>) -> Result<Automation, FakeError> {
        let automation = Automation {
            id: new.id,
            team_id: new.team_id,
            created_by: new.created_by.clone(),
            name: new.name.to_string(),
            trigger: new.trigger.clone(),
            flow: new.flow.to_string(),
            repo_url: new.repo_url.map(str::to_string),
            request_template: new.request_template.to_string(),
            enabled: true,
            routine_id: None,
            routine_revision: None,
            created_at: Utc::now(),
        };
        self.automations.lock().expect("automations").insert(
            new.id,
            (automation.clone(), new.webhook_secret.map(str::to_string)),
        );
        Ok(automation)
    }

    async fn set_automation_routine(
        &self,
        id: Uuid,
        routine_id: Uuid,
        revision: i64,
    ) -> Result<(), FakeError> {
        if let Some((automation, _)) = self.automations.lock().expect("automations").get_mut(&id) {
            automation.routine_id = Some(routine_id);
            automation.routine_revision = Some(revision);
        }
        Ok(())
    }

    async fn automation(&self, id: Uuid) -> Result<Option<Automation>, FakeError> {
        Ok(self
            .automations
            .lock()
            .expect("automations")
            .get(&id)
            .map(|(automation, _)| automation.clone()))
    }

    async fn automation_of_routine(
        &self,
        routine_id: Uuid,
    ) -> Result<Option<Automation>, FakeError> {
        Ok(self
            .automations
            .lock()
            .expect("automations")
            .values()
            .map(|(automation, _)| automation)
            .find(|automation| automation.routine_id == Some(routine_id))
            .cloned())
    }

    async fn team_automations(
        &self,
        team_id: Uuid,
        limit: u16,
    ) -> Result<Vec<Automation>, FakeError> {
        let mut automations: Vec<Automation> = self
            .automations
            .lock()
            .expect("automations")
            .values()
            .map(|(automation, _)| automation)
            .filter(|automation| automation.team_id == team_id)
            .cloned()
            .collect();
        automations.sort_by(|a, b| b.id.cmp(&a.id));
        automations.truncate(usize::from(limit));
        Ok(automations)
    }

    async fn webhook_secret(&self, id: Uuid) -> Result<Option<String>, FakeError> {
        Ok(self
            .automations
            .lock()
            .expect("automations")
            .get(&id)
            .and_then(|(_, secret)| secret.clone()))
    }

    async fn set_automation_enabled(
        &self,
        id: Uuid,
        enabled: bool,
        routine_revision: Option<i64>,
    ) -> Result<bool, FakeError> {
        let mut automations = self.automations.lock().expect("automations");
        let Some((automation, _)) = automations.get_mut(&id) else {
            return Ok(false);
        };
        automation.enabled = enabled;
        automation.routine_revision = routine_revision.or(automation.routine_revision);
        Ok(true)
    }

    async fn delete_automation(&self, id: Uuid) -> Result<(), FakeError> {
        self.automations.lock().expect("automations").remove(&id);
        self.firings
            .lock()
            .expect("firings")
            .retain(|(automation_id, _), _| *automation_id != id);
        Ok(())
    }

    async fn claim_firing(&self, automation_id: Uuid, key: &str) -> Result<FiringClaim, FakeError> {
        let mut firings = self.firings.lock().expect("firings");
        match firings.get(&(automation_id, key.to_string())) {
            None => {
                firings.insert((automation_id, key.to_string()), None);
                Ok(FiringClaim::Claimed)
            }
            Some(Some(end)) => Ok(FiringClaim::Ended(*end)),
            Some(None) => Ok(FiringClaim::InProgress),
        }
    }

    async fn end_firing(
        &self,
        automation_id: Uuid,
        key: &str,
        end: FiringEnd,
    ) -> Result<(), FakeError> {
        if let Some(firing @ None) = self
            .firings
            .lock()
            .expect("firings")
            .get_mut(&(automation_id, key.to_string()))
        {
            *firing = Some(end);
        }
        Ok(())
    }

    async fn running_run_on_task(&self, task_id: Uuid) -> Result<Option<RunId>, FakeError> {
        Ok(self
            .runs
            .lock()
            .expect("runs")
            .values()
            .find(|run| run.task_id == Some(task_id) && run.end.is_none())
            .map(|run| run.id))
    }
}
