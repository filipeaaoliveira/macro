//! The Postgres Run repository's Machines: the harness an agent's bot is
//! bound to, and which Step holds a slot on it (`factory_step.machine_id`).

#[cfg(test)]
mod test;

use std::time::Duration;

use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::machines::{MachineRepository, SLOT_HELD_AT_MOST, SlotTake, StepMachine};
use crate::domain::models::StepKey;

impl MachineRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn bot_machine(&self, bot: Uuid) -> Result<Option<StepMachine>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT h.id, h.name, h.step_slots, h.tags
            FROM agent_configs c
            JOIN harnesses h ON h.id = c.harness_id AND h.deleted_at IS NULL
            WHERE c.bot_id = $1
            "#,
            bot,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| StepMachine {
            id: row.id,
            name: row.name,
            // The table keeps 1 to 64.
            step_slots: row.step_slots.and_then(|slots| u16::try_from(slots).ok()),
            tags: row.tags,
        }))
    }

    #[tracing::instrument(err, skip(self))]
    async fn take_machine_slot(
        &self,
        key: StepKey,
        machine: Uuid,
        take_within: Duration,
    ) -> Result<SlotTake, Self::Err> {
        // One lock per Machine, held to the commit: the count and the take
        // are one decision, so two Steps never both take its last slot.
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "SELECT pg_advisory_xact_lock(hashtextextended('factory-machine-slot:' || $1, 0))",
            machine.to_string(),
        )
        .execute(&mut *tx)
        .await?;

        // A Step that holds this Machine's slot keeps it. A Step not
        // recorded has nothing to hold: the take answers Taken and changes
        // nothing, and its claim then finds out why.
        let step = sqlx::query!(
            r#"
            SELECT
                machine_id IS NOT DISTINCT FROM $3 AS "holds!",
                now() - started_at > make_interval(secs => $4) AS "late!"
            FROM factory_step
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            machine,
            take_within.as_secs_f64(),
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(step) = step else {
            tx.rollback().await?;
            return Ok(SlotTake::Taken);
        };
        if step.holds {
            tx.rollback().await?;
            return Ok(SlotTake::Taken);
        }

        // Running Steps hold the slots: those that have not ended, in Runs
        // that have not ended, started within SLOT_HELD_AT_MOST. An older
        // one's work can no longer run, as when its Run's workflow is gone
        // without recording its end.
        let free = sqlx::query_scalar!(
            r#"
            SELECT
                h.step_slots IS NULL
                OR h.step_slots > (
                    SELECT count(*)
                    FROM factory_step s
                    JOIN factory_run r ON r.id = s.run_id
                    WHERE s.machine_id = h.id
                        AND s.ended_at IS NULL
                        AND r.ended_at IS NULL
                        AND s.started_at > now() - make_interval(secs => $4)
                        AND NOT (s.run_id = $2 AND s.seq = $3)
                ) AS "free!"
            FROM harnesses h
            WHERE h.id = $1
            "#,
            machine,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            SLOT_HELD_AT_MOST.as_secs_f64(),
        )
        .fetch_optional(&mut *tx)
        .await?
        // A Machine removed since it was read has no limit to keep: its
        // session is refused when it opens.
        .unwrap_or(true);
        // A full Machine is not worth waiting for once the Step started too
        // long ago; a free slot is taken however late.
        if !free {
            tx.rollback().await?;
            if step.late {
                return Ok(SlotTake::TooLate);
            }
            return Ok(SlotTake::Full);
        }

        sqlx::query!(
            r#"
            UPDATE factory_step
            SET machine_id = $3, slot_wait_since = NULL, slot_wait_machines = NULL
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            machine,
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(SlotTake::Taken)
    }

    #[tracing::instrument(err, skip(self))]
    async fn session_opened(&self, key: StepKey) -> Result<bool, Self::Err> {
        let opened = sqlx::query_scalar!(
            r#"
            SELECT opened_at IS NOT NULL AS "opened!"
            FROM factory_step
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(opened.unwrap_or(false))
    }

    #[tracing::instrument(err, skip(self))]
    async fn release_machine_slot(&self, key: StepKey) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET machine_id = NULL, slot_wait_since = NULL, slot_wait_machines = NULL
            WHERE run_id = $1 AND seq = $2
                AND (machine_id IS NOT NULL OR slot_wait_since IS NOT NULL)
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn wait_for_machine_slot(
        &self,
        key: StepKey,
        machines: &[String],
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET slot_wait_since = COALESCE(slot_wait_since, now()),
                slot_wait_machines = $3
            WHERE run_id = $1 AND seq = $2 AND ended_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            machines,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
