//! The Postgres Run repository's Machines: the harness an agent's bot is
//! bound to, and which Step holds a slot on it (`factory_step.machine_id`).

#[cfg(test)]
mod test;

use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::machines::{MachineRepository, StepMachine};
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
    async fn take_machine_slot(&self, key: StepKey, machine: Uuid) -> Result<bool, Self::Err> {
        // One lock per Machine, held to the commit: the count and the take
        // are one decision, so two Steps never both take its last slot. A
        // Step of a Run that ended holds none, should its own end be missing.
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            "SELECT pg_advisory_xact_lock(hashtextextended('factory-machine-slot:' || $1, 0))",
            machine.to_string(),
        )
        .execute(&mut *tx)
        .await?;
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
                        AND NOT (s.run_id = $2 AND s.seq = $3)
                ) AS "free!"
            FROM harnesses h
            WHERE h.id = $1
            "#,
            machine,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&mut *tx)
        .await?
        // A Machine removed since it was read has no limit to keep: its
        // session is refused when it opens.
        .unwrap_or(true);
        if !free {
            return Ok(false);
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
        Ok(true)
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
