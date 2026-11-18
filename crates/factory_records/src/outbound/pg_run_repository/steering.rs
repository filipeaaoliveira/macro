//! The Postgres Run repository's Step Terminal: what the Run's owner did to
//! a running Step (`factory_step_steer`), and the steering its work reads
//! (`factory_step`'s `paused_at`, `stop_asked_at`, `steer_action_id` and
//! `steer_closed`).

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::models::{
    AgentName, NewSteer, Role, RunId, Steer, SteerAction, SteerRefusal, Steered, StepKey,
    StepSession, StepSteering,
};

impl PgRunRepository {
    /// [`crate::domain::ports::RunRepository::steer`]: under the Step's row
    /// lock, so the steering it checks is the one it changes, and a steer's
    /// place follows the one before it.
    pub(super) async fn record_steer(
        &self,
        key: StepKey,
        steer: &NewSteer,
    ) -> Result<Result<Steered, SteerRefusal>, PgRunRepositoryErr> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query!(
            r#"
            SELECT
                s.session_id, s.action_id, s.agent, s.bot_id, s.role,
                s.ended_at, s.session_released_at, s.paused_at, s.stop_asked_at,
                s.steer_action_id, s.steer_closed, r.ended_at AS run_ended_at
            FROM factory_step s
            JOIN factory_run r ON r.id = s.run_id
            WHERE s.run_id = $1 AND s.seq = $2
            FOR UPDATE OF s
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(Err(SteerRefusal::Ended));
        };
        if row.ended_at.is_some() || row.session_released_at.is_some() || row.run_ended_at.is_some()
        {
            return Ok(Err(SteerRefusal::Ended));
        }
        let (Some(session_id), Some(action_id), Some(role)) =
            (row.session_id, row.action_id, row.role)
        else {
            return Ok(Err(SteerRefusal::NoSession));
        };
        let paused = row.paused_at.is_some();
        let refusal = if row.stop_asked_at.is_some() {
            Some(SteerRefusal::Stopping)
        } else {
            match steer {
                NewSteer::Note { .. } | NewSteer::Pause if paused => Some(SteerRefusal::Paused),
                NewSteer::Resume { .. } if !paused => Some(SteerRefusal::NotPaused),
                // A paused Step's work still waits on its agent, so it never
                // stopped taking steering.
                _ if row.steer_closed => Some(SteerRefusal::NotWorking),
                _ => None,
            }
        };
        if let Some(refusal) = refusal {
            return Ok(Err(refusal));
        }

        let action = steer.action();
        let note = match steer {
            NewSteer::Note { note, .. } => Some(note.as_str()),
            NewSteer::Resume { note, .. } => note.as_deref(),
            NewSteer::Pause | NewSteer::Stop => None,
        };
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET paused_at = CASE $3
                    WHEN 'pause' THEN now()
                    WHEN 'resume' THEN NULL
                    ELSE paused_at
                END,
                stop_asked_at = CASE WHEN $3 = 'stop' THEN now() ELSE stop_asked_at END,
                steer_action_id = COALESCE($4, steer_action_id)
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            action.as_str(),
            steer.prompt_id(),
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!(
            r#"
            INSERT INTO factory_step_steer (run_id, seq, place, action, note, prompt_id)
            SELECT $1, $2, COALESCE(MAX(place), 0) + 1, $3, $4, $5
            FROM factory_step_steer
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            action.as_str(),
            note,
            steer.prompt_id(),
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        Ok(Ok(Steered {
            session: StepSession {
                session_id,
                action_id,
                agent: row.agent.map(AgentName::try_from).transpose()?,
                bot: row.bot_id,
            },
            role: Role::try_from(role)?,
            previous_prompt: row.steer_action_id,
        }))
    }

    /// [`crate::domain::ports::RunRepository::unsteer`].
    pub(super) async fn undo_steer(
        &self,
        key: StepKey,
        prompt_id: Uuid,
        previous: Option<Uuid>,
    ) -> Result<(), PgRunRepositoryErr> {
        let mut tx = self.pool.begin().await?;
        let resumed = sqlx::query_scalar!(
            r#"
            UPDATE factory_step_steer
            SET undelivered = true
            WHERE run_id = $1 AND seq = $2 AND prompt_id = $3
            RETURNING action = 'resume' AS "resumed!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            prompt_id,
        )
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(false);
        // A resume the agent never read leaves the Step as it was: paused.
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET steer_action_id = $4,
                paused_at = CASE WHEN $5 THEN COALESCE(paused_at, now()) ELSE paused_at END
            WHERE run_id = $1 AND seq = $2 AND steer_action_id = $3 AND ended_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            prompt_id,
            previous,
            resumed,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// [`crate::domain::ports::RunRepository::steering`]: a Step that is not
    /// there has none.
    pub(super) async fn read_steering(
        &self,
        key: StepKey,
    ) -> Result<StepSteering, PgRunRepositoryErr> {
        let row = sqlx::query!(
            r#"
            SELECT
                paused_at IS NOT NULL AS "paused!",
                stop_asked_at IS NOT NULL AS "stop_asked!",
                steer_action_id
            FROM factory_step
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map_or_else(StepSteering::default, |row| StepSteering {
            paused: row.paused,
            stop_asked: row.stop_asked,
            prompt: row.steer_action_id,
        }))
    }

    /// [`crate::domain::ports::RunRepository::close_steering`]: a Step that
    /// is not there takes no steering, so its work has nothing to follow.
    pub(super) async fn stop_steering(
        &self,
        key: StepKey,
        followed: Option<Uuid>,
    ) -> Result<bool, PgRunRepositoryErr> {
        let closed = sqlx::query_scalar!(
            r#"
            WITH closed AS (
                UPDATE factory_step
                SET steer_closed = true
                WHERE run_id = $1 AND seq = $2
                    AND steer_action_id IS NOT DISTINCT FROM $3
                    AND paused_at IS NULL AND stop_asked_at IS NULL
                RETURNING 1
            )
            SELECT EXISTS (SELECT 1 FROM closed)
                OR NOT EXISTS (SELECT 1 FROM factory_step WHERE run_id = $1 AND seq = $2)
                AS "closed!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            followed,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(closed)
    }

    /// The steers of the Runs `ids`' Steps, oldest first, by Step.
    pub(super) async fn runs_steers(
        &self,
        ids: &[Uuid],
    ) -> Result<BTreeMap<(RunId, u16), Vec<Steer>>, PgRunRepositoryErr> {
        let rows = sqlx::query!(
            r#"
            SELECT run_id, seq, action, note, undelivered, at
            FROM factory_step_steer
            WHERE run_id = ANY($1)
            ORDER BY run_id, seq, place
            "#,
            ids,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut steers: BTreeMap<(RunId, u16), Vec<Steer>> = BTreeMap::new();
        for row in rows {
            let seq = u16::try_from(row.seq).map_err(|_| PgRunRepositoryErr::OutOfRange(row.seq))?;
            let at: DateTime<Utc> = row.at;
            steers
                .entry((RunId::from_uuid(row.run_id), seq))
                .or_default()
                .push(Steer {
                    action: row.action.parse::<SteerAction>()?,
                    note: row.note,
                    undelivered: row.undelivered,
                    at,
                });
        }
        Ok(steers)
    }
}
