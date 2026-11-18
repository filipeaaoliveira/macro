//! The Postgres Run repository's standing acts: `factory_standing_act`, and
//! the bot behind an agent session (`agent_session.bot_id`).

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::models::{AgentName, RunId};
use crate::domain::standing_acts::{
    StandingAct, StandingActRecord, StandingActRepository, StandingActor, StandingRoleName,
};

impl StandingActRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, owner))]
    async fn session_bot(
        &self,
        session_id: Uuid,
        owner: &MacroUserIdStr<'static>,
    ) -> Result<Option<Uuid>, Self::Err> {
        let bot = sqlx::query_scalar!(
            "SELECT bot_id FROM agent_session WHERE id = $1 AND owner_id = $2",
            session_id,
            owner.as_ref(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(bot)
    }

    #[tracing::instrument(err, skip(self, actor))]
    async fn record_standing_act(
        &self,
        run_id: RunId,
        act: StandingAct,
        seq: Option<u16>,
        actor: &StandingActor,
    ) -> Result<(), Self::Err> {
        // A start, a cancel or an answer done again (a retried activity, a
        // second cancel) keeps the first: the unique index takes one.
        sqlx::query!(
            r#"
            INSERT INTO factory_standing_act
                (run_id, act, seq, bot_id, agent, standing_role, session_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT DO NOTHING
            "#,
            run_id.as_uuid(),
            act.as_ref(),
            seq.map(i32::from),
            actor.bot_id,
            actor.agent.as_str(),
            actor.standing_role.as_str(),
            actor.session_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn runs_standing_acts(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<StandingActRecord>>, Self::Err> {
        let ids: Vec<Uuid> = ids.iter().map(RunId::as_uuid).collect();
        let rows = sqlx::query!(
            r#"
            SELECT run_id, act, seq, bot_id, agent, standing_role, session_id, at
            FROM factory_standing_act
            WHERE run_id = ANY($1)
            ORDER BY run_id, at, id
            "#,
            &ids,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut acts: BTreeMap<RunId, Vec<StandingActRecord>> = BTreeMap::new();
        for row in rows {
            let act = row
                .act
                .parse::<StandingAct>()
                .map_err(|_| PgRunRepositoryErr::UnknownStandingAct(row.act.clone()))?;
            let seq = row
                .seq
                .map(|seq| u16::try_from(seq).map_err(|_| PgRunRepositoryErr::OutOfRange(seq)))
                .transpose()?;
            acts.entry(RunId::from_uuid(row.run_id))
                .or_default()
                .push(StandingActRecord {
                    act,
                    seq,
                    bot_id: row.bot_id,
                    agent: AgentName::try_from(row.agent)?,
                    standing_role: StandingRoleName::try_from(row.standing_role)?,
                    session_id: row.session_id,
                    at: row.at,
                });
        }
        Ok(acts)
    }
}
