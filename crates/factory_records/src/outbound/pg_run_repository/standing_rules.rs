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

/// The definition whose rules decide the stored `kinds`.
fn definition(kinds: Vec<String>) -> Result<OrchestratorDefinition, PgRunRepositoryErr> {
    OrchestratorDefinition::from_kinds(&kinds)
        .ok_or(PgRunRepositoryErr::UnknownStandingRules(kinds))
}

impl StandingRulesRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn team_rules(&self, team_id: Uuid) -> Result<Option<OrchestratorDefinition>, Self::Err> {
        let kinds = sqlx::query_scalar!(
            "SELECT standing_rules FROM factory_team_settings WHERE team_id = $1",
            team_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        kinds.flatten().map(definition).transpose()
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
        let kinds = definition.map(OrchestratorDefinition::kinds);
        let mut transaction = self.pool.begin().await?;
        sqlx::query!(
            r#"
            INSERT INTO factory_team_settings (team_id, standing_rules)
            VALUES ($1, $2)
            ON CONFLICT (team_id)
            DO UPDATE SET standing_rules = EXCLUDED.standing_rules, updated_at = now()
            "#,
            team_id,
            kinds.as_deref(),
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
        let kinds = sqlx::query_scalar!(
            r#"
            SELECT standing_rules FROM factory_project_rules
            WHERE team_id = $1 AND project_id = $2
            "#,
            team_id,
            project_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        kinds.map(definition).transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn list_project_rules(&self, team_id: Uuid) -> Result<Vec<ProjectRules>, Self::Err> {
        let rows = sqlx::query!(
            r#"
            SELECT project_id, standing_rules FROM factory_project_rules
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
                    definition: definition(row.standing_rules)?,
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
        let set = sqlx::query!(
            r#"
            INSERT INTO factory_project_rules (team_id, project_id, standing_rules)
            VALUES ($1, $2, $3)
            ON CONFLICT (team_id, project_id)
            DO UPDATE SET standing_rules = EXCLUDED.standing_rules, updated_at = now()
            "#,
            team_id,
            project_id,
            &definition.kinds(),
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
