//! Teams' Automations (`factory_automation`) and each time one fired
//! (`factory_automation_firing`).

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::types::Json;
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::automations::{
    Automation, AutomationRepository, AutomationTrigger, FIRING_CLAIM_STALE_AFTER, FiringClaim,
    FiringEnd, FiringSkip, NewAutomation, TriggerKind,
};
use crate::domain::models::RunId;

struct AutomationRow {
    id: Uuid,
    team_id: Uuid,
    created_by: String,
    name: String,
    trigger_kind: String,
    trigger: Json<serde_json::Value>,
    flow: String,
    repo_url: Option<String>,
    request_template: String,
    enabled: bool,
    routine_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

impl TryFrom<AutomationRow> for Automation {
    type Error = PgRunRepositoryErr;

    fn try_from(row: AutomationRow) -> Result<Self, Self::Error> {
        let trigger: AutomationTrigger =
            serde_json::from_value(row.trigger.0).map_err(PgRunRepositoryErr::UnreadableTrigger)?;
        // The kind is kept in a column of its own for the table's checks;
        // the two never disagree.
        let kind = row
            .trigger_kind
            .parse::<TriggerKind>()
            .map_err(|_| PgRunRepositoryErr::UnknownTriggerKind(row.trigger_kind.clone()))?;
        if kind != trigger.kind() {
            return Err(PgRunRepositoryErr::UnknownTriggerKind(row.trigger_kind));
        }
        Ok(Automation {
            id: row.id,
            team_id: row.team_id,
            created_by: MacroUserIdStr::try_from(row.created_by.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.created_by))?,
            name: row.name,
            trigger,
            flow: row.flow,
            repo_url: row.repo_url,
            request_template: row.request_template,
            enabled: row.enabled,
            routine_id: row.routine_id,
            created_at: row.created_at,
        })
    }
}

/// What a firing's row says it came to.
fn firing_end(
    run_id: Option<Uuid>,
    skipped: Option<String>,
) -> Result<Option<FiringEnd>, PgRunRepositoryErr> {
    match (run_id, skipped) {
        (Some(run_id), _) => Ok(Some(FiringEnd::Started(RunId::from_uuid(run_id)))),
        (None, Some(skipped)) => skipped
            .parse::<FiringSkip>()
            .map(|skip| Some(FiringEnd::Skipped(skip)))
            .map_err(|_| PgRunRepositoryErr::UnreadableFiring),
        (None, None) => Ok(None),
    }
}

impl AutomationRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, automation), fields(id = %automation.id))]
    async fn insert_automation(
        &self,
        automation: NewAutomation<'_>,
    ) -> Result<Automation, Self::Err> {
        let kind = automation.trigger.kind();
        let trigger = serde_json::to_value(automation.trigger)
            .map_err(PgRunRepositoryErr::UnreadableTrigger)?;
        let row = sqlx::query_as!(
            AutomationRow,
            r#"
            INSERT INTO factory_automation (
                id, team_id, created_by, name, trigger_kind, trigger, flow, repo_url,
                request_template, webhook_secret
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            RETURNING
                id, team_id, created_by, name, trigger_kind,
                trigger AS "trigger: Json<serde_json::Value>", flow, repo_url, request_template,
                enabled, routine_id, created_at
            "#,
            automation.id,
            automation.team_id,
            automation.created_by.as_ref(),
            automation.name,
            kind.as_ref(),
            trigger,
            automation.flow,
            automation.repo_url,
            automation.request_template,
            automation.webhook_secret,
        )
        .fetch_one(&self.pool)
        .await?;
        Automation::try_from(row)
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_automation_routine(&self, id: Uuid, routine_id: Uuid) -> Result<(), Self::Err> {
        sqlx::query!(
            "UPDATE factory_automation SET routine_id = $2 WHERE id = $1",
            id,
            routine_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn automation(&self, id: Uuid) -> Result<Option<Automation>, Self::Err> {
        let row = sqlx::query_as!(
            AutomationRow,
            r#"
            SELECT
                id, team_id, created_by, name, trigger_kind,
                trigger AS "trigger: Json<serde_json::Value>", flow, repo_url, request_template,
                enabled, routine_id, created_at
            FROM factory_automation
            WHERE id = $1
            "#,
            id,
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Automation::try_from).transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn automation_of_routine(
        &self,
        routine_id: Uuid,
    ) -> Result<Option<Automation>, Self::Err> {
        let row = sqlx::query_as!(
            AutomationRow,
            r#"
            SELECT
                id, team_id, created_by, name, trigger_kind,
                trigger AS "trigger: Json<serde_json::Value>", flow, repo_url, request_template,
                enabled, routine_id, created_at
            FROM factory_automation
            WHERE routine_id = $1
            "#,
            routine_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Automation::try_from).transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn team_automations(
        &self,
        team_id: Uuid,
        limit: u16,
    ) -> Result<Vec<Automation>, Self::Err> {
        // `factory_automation_team_idx` reads the team's newest first and
        // stops at the limit.
        let rows = sqlx::query_as!(
            AutomationRow,
            r#"
            SELECT
                id, team_id, created_by, name, trigger_kind,
                trigger AS "trigger: Json<serde_json::Value>", flow, repo_url, request_template,
                enabled, routine_id, created_at
            FROM factory_automation
            WHERE team_id = $1
            ORDER BY created_at DESC, id DESC
            LIMIT $2
            "#,
            team_id,
            i64::from(limit),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Automation::try_from).collect()
    }

    #[tracing::instrument(err, skip(self))]
    async fn webhook_secret(&self, id: Uuid) -> Result<Option<String>, Self::Err> {
        let secret = sqlx::query_scalar!(
            "SELECT webhook_secret FROM factory_automation WHERE id = $1",
            id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(secret.flatten())
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_automation_enabled(&self, id: Uuid, enabled: bool) -> Result<bool, Self::Err> {
        let done = sqlx::query!(
            "UPDATE factory_automation SET enabled = $2 WHERE id = $1",
            id,
            enabled,
        )
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() > 0)
    }

    #[tracing::instrument(err, skip(self))]
    async fn delete_automation(&self, id: Uuid) -> Result<(), Self::Err> {
        sqlx::query!("DELETE FROM factory_automation WHERE id = $1", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    // A key is claimed by the insert; a firing found under it that never
    // ended is taken over only once its claim is stale, at the database's
    // time, so two workers' clocks never decide it.
    #[tracing::instrument(err, skip(self, key))]
    async fn claim_firing(&self, automation_id: Uuid, key: &str) -> Result<FiringClaim, Self::Err> {
        let stale_after = sqlx::postgres::types::PgInterval::try_from(FIRING_CLAIM_STALE_AFTER)
            .map_err(|_| PgRunRepositoryErr::UnreadableFiring)?;
        let claimed = sqlx::query_scalar!(
            r#"
            INSERT INTO factory_automation_firing (automation_id, firing_key)
            VALUES ($1, $2)
            ON CONFLICT (automation_id, firing_key) DO UPDATE
                SET fired_at = now()
                WHERE factory_automation_firing.run_id IS NULL
                    AND factory_automation_firing.skipped IS NULL
                    AND factory_automation_firing.fired_at < now() - $3::interval
            RETURNING true AS "claimed!"
            "#,
            automation_id,
            key,
            stale_after,
        )
        .fetch_optional(&self.pool)
        .await?;
        if claimed.is_some() {
            return Ok(FiringClaim::Claimed);
        }

        let firing = sqlx::query!(
            r#"
            SELECT run_id, skipped
            FROM factory_automation_firing
            WHERE automation_id = $1 AND firing_key = $2
            "#,
            automation_id,
            key,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(firing) = firing else {
            // Its Automation was deleted between the two reads.
            return Ok(FiringClaim::Ended(FiringEnd::Skipped(FiringSkip::Disabled)));
        };
        Ok(match firing_end(firing.run_id, firing.skipped)? {
            Some(end) => FiringClaim::Ended(end),
            None => FiringClaim::InProgress,
        })
    }

    #[tracing::instrument(err, skip(self, key))]
    async fn end_firing(
        &self,
        automation_id: Uuid,
        key: &str,
        end: FiringEnd,
    ) -> Result<(), Self::Err> {
        let (run_id, skipped) = match end {
            FiringEnd::Started(run_id) => (Some(run_id.as_uuid()), None),
            FiringEnd::Skipped(skip) => (None, Some(skip)),
        };
        let skipped = skipped.as_ref().map(|skip| skip.as_ref());
        sqlx::query!(
            r#"
            UPDATE factory_automation_firing
            SET run_id = $3, skipped = $4
            WHERE automation_id = $1 AND firing_key = $2 AND run_id IS NULL AND skipped IS NULL
            "#,
            automation_id,
            key,
            run_id,
            skipped,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn running_run_on_task(&self, task_id: Uuid) -> Result<Option<RunId>, Self::Err> {
        // `factory_run_task_owner_idx` leads with the task.
        let id = sqlx::query_scalar!(
            r#"
            SELECT id FROM factory_run
            WHERE task_id = $1 AND ended_at IS NULL
            LIMIT 1
            "#,
            task_id.to_string(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(id.map(RunId::from_uuid))
    }
}
