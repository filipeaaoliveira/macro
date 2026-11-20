//! Teams' Standing Rules: their own, in `factory_team_settings` beside
//! their Control Files, and their Projects', in `factory_project_rules`.

#[cfg(test)]
mod test;

use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::standing_rules::{
    OrchestratorDefinition, ProjectRules, ProjectRulesSet, StandingRulesRepository,
};

/// The foreign key from a Project's rules to its initiative.
const PROJECT_FOREIGN_KEY: &str = "factory_project_rules_project_id_fkey";

/// The definition whose rules decide the stored `kinds`, with the stored
/// Queue limit and token budget.
fn definition(
    kinds: Vec<String>,
    queue_limit: Option<i16>,
    token_budget: Option<i64>,
) -> Result<OrchestratorDefinition, PgRunRepositoryErr> {
    let queue_limit = queue_limit
        .map(|limit| u16::try_from(limit).map_err(|_| PgRunRepositoryErr::OutOfRange(limit.into())))
        .transpose()?;
    let token_budget = token_budget
        .map(|budget| u64::try_from(budget).map_err(|_| PgRunRepositoryErr::BudgetOutOfRange))
        .transpose()?;
    let definition = OrchestratorDefinition::from_kinds(&kinds)
        .ok_or(PgRunRepositoryErr::UnknownStandingRules(kinds))?;
    Ok(OrchestratorDefinition {
        queue_limit,
        token_budget,
        ..definition
    })
}

/// How the token budget of `definition` is stored.
fn stored_token_budget(
    definition: Option<&OrchestratorDefinition>,
) -> Result<Option<i64>, PgRunRepositoryErr> {
    definition
        .and_then(|definition| definition.token_budget)
        .map(|budget| i64::try_from(budget).map_err(|_| PgRunRepositoryErr::BudgetOutOfRange))
        .transpose()
}

/// How the Queue limit of `definition` is stored.
fn stored_queue_limit(
    definition: Option<&OrchestratorDefinition>,
) -> Result<Option<i16>, PgRunRepositoryErr> {
    definition
        .and_then(|definition| definition.queue_limit)
        .map(|limit| {
            i16::try_from(limit).map_err(|_| PgRunRepositoryErr::OutOfRange(i32::from(limit)))
        })
        .transpose()
}

impl StandingRulesRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn team_rules(&self, team_id: Uuid) -> Result<Option<OrchestratorDefinition>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT standing_rules, queue_limit, token_budget FROM factory_team_settings
            WHERE team_id = $1
            "#,
            team_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        row.standing_rules
            .map(|kinds| definition(kinds, row.queue_limit, row.token_budget))
            .transpose()
    }

    // The row also keeps the team's Control Files: going back to the
    // default rules clears them from it, and removes a row left with
    // neither setting.
    #[tracing::instrument(err, skip(self))]
    async fn set_team_rules(
        &self,
        team_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> Result<(), Self::Err> {
        let queue_limit = stored_queue_limit(definition.as_ref())?;
        let token_budget = stored_token_budget(definition.as_ref())?;
        let kinds = definition.map(OrchestratorDefinition::kinds);
        let mut transaction = self.pool.begin().await?;
        sqlx::query!(
            r#"
            INSERT INTO factory_team_settings (team_id, standing_rules, queue_limit, token_budget)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (team_id)
            DO UPDATE SET
                standing_rules = EXCLUDED.standing_rules,
                queue_limit = EXCLUDED.queue_limit,
                token_budget = EXCLUDED.token_budget,
                updated_at = now()
            "#,
            team_id,
            kinds.as_deref(),
            queue_limit,
            token_budget,
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query!(
            r#"
            DELETE FROM factory_team_settings
            WHERE team_id = $1 AND control_files IS NULL AND standing_rules IS NULL
            "#,
            team_id,
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn project_rules(
        &self,
        team_id: Uuid,
        project_id: Uuid,
    ) -> Result<Option<OrchestratorDefinition>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT standing_rules, queue_limit, token_budget FROM factory_project_rules
            WHERE team_id = $1 AND project_id = $2
            "#,
            team_id,
            project_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| definition(row.standing_rules, row.queue_limit, row.token_budget))
            .transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn list_project_rules(&self, team_id: Uuid) -> Result<Vec<ProjectRules>, Self::Err> {
        let rows = sqlx::query!(
            r#"
            SELECT project_id, standing_rules, queue_limit, token_budget FROM factory_project_rules
            WHERE team_id = $1
            ORDER BY project_id
            "#,
            team_id,
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ProjectRules {
                    project_id: row.project_id,
                    definition: definition(row.standing_rules, row.queue_limit, row.token_budget)?,
                })
            })
            .collect()
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_project_rules(
        &self,
        team_id: Uuid,
        project_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> Result<ProjectRulesSet, Self::Err> {
        let Some(definition) = definition else {
            sqlx::query!(
                "DELETE FROM factory_project_rules WHERE team_id = $1 AND project_id = $2",
                team_id,
                project_id,
            )
            .execute(&self.pool)
            .await?;
            return Ok(ProjectRulesSet::Set);
        };
        let queue_limit = stored_queue_limit(Some(&definition))?;
        let token_budget = stored_token_budget(Some(&definition))?;
        let set = sqlx::query!(
            r#"
            INSERT INTO factory_project_rules (
                team_id, project_id, standing_rules, queue_limit, token_budget
            )
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (team_id, project_id)
            DO UPDATE SET
                standing_rules = EXCLUDED.standing_rules,
                queue_limit = EXCLUDED.queue_limit,
                token_budget = EXCLUDED.token_budget,
                updated_at = now()
            "#,
            team_id,
            project_id,
            &definition.kinds(),
            queue_limit,
            token_budget,
        )
        .execute(&self.pool)
        .await;
        match set {
            Ok(_) => Ok(ProjectRulesSet::Set),
            Err(sqlx::Error::Database(error))
                if error.constraint() == Some(PROJECT_FOREIGN_KEY) =>
            {
                Ok(ProjectRulesSet::NoSuchProject)
            }
            Err(error) => Err(error.into()),
        }
    }
}
