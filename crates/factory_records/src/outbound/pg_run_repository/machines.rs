//! The Postgres Run repository's Machines: the harness an agent's bot is
//! bound to, and which Step holds a slot on it (`factory_step.machine_id`).

#[cfg(test)]
mod test;

use std::time::Duration;

use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::machines::{
    MachineLogin, MachineRepository, OwnerBot, OwnerBotRuns, SLOT_HELD_AT_MOST, SlotTake,
    StepMachine, Vendor,
};
use crate::domain::models::StepKey;

impl MachineRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self))]
    async fn bot_machine(&self, bot: Uuid) -> Result<Option<StepMachine>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT h.id, h.name, h.step_slots, h.tags,
                COALESCE(h.owner_user_id, h.created_by) AS "owner!",
                h.computer_id IS NOT NULL AS "on_computer!",
                EXISTS (
                    SELECT 1 FROM harness_account_logins login
                    WHERE login.harness_id = h.id AND login.signed_out_at IS NULL
                ) AS "signed_in!"
            FROM agent_configs c
            JOIN harnesses h ON h.id = c.harness_id AND h.deleted_at IS NULL
            WHERE c.bot_id = $1
            "#,
            bot,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| {
            // A Macro computer runs on Macro credits through the gateway
            // unless a provider sign-in was made on it through Macro; a
            // macrod machine runs its CLI on its owner's own login.
            let login = match (row.on_computer, row.signed_in) {
                (false, _) => MachineLogin::Owner(row.owner),
                (true, false) => MachineLogin::MacroCredits,
                (true, true) => MachineLogin::HostedSignIn,
            };
            StepMachine {
                id: row.id,
                name: row.name,
                // The table keeps 1 to 64.
                step_slots: row.step_slots.and_then(|slots| u16::try_from(slots).ok()),
                tags: row.tags,
                login,
            }
        }))
    }

    #[tracing::instrument(err, skip(self))]
    async fn owner_bots(&self, owner: &str) -> Result<Vec<OwnerBot>, Self::Err> {
        // Three kinds, in the order a Step takes them (`place`): the coding
        // agents on macrod machines the owner paired (no Macro computer, no
        // machine only shared with them), the owner's own in-memory agents
        // with a key, then those of the owner's teams. A user's machines and
        // agents are few: each kind reads through its owner's index.
        let rows = sqlx::query!(
            r#"
            SELECT bot_id AS "bot_id!", model AS "model!", place AS "place!", provider, team_id
            FROM (
                SELECT b.id AS bot_id, c.default_model AS model, 0 AS place,
                    NULL::text AS provider, NULL::uuid AS team_id
                FROM harnesses h
                JOIN agent_configs c ON c.harness_id = h.id AND c.is_coding
                JOIN bots b ON b.id = c.bot_id AND b.deleted_at IS NULL
                WHERE h.owner_user_id = $1
                    AND h.deleted_at IS NULL
                    AND h.computer_id IS NULL
                    -- A teammate's private agent bound to the owner's
                    -- machine is not the owner's to run.
                    AND (
                        b.owner_user_id = $1
                        OR b.team_id IN (SELECT team_id FROM team_user WHERE user_id = $1)
                    )
                UNION ALL
                SELECT b.id, c.default_model,
                    CASE WHEN b.owner_user_id = $1 THEN 1 ELSE 2 END,
                    k.provider, b.team_id
                FROM bots b
                JOIN agent_api_keys k ON k.bot_id = b.id
                JOIN agent_configs c ON c.bot_id = b.id
                    AND c.harness IN ('in-memory', 'macro-inmem')
                WHERE b.deleted_at IS NULL
                    AND (
                        b.owner_user_id = $1
                        OR b.team_id IN (SELECT team_id FROM team_user WHERE user_id = $1)
                    )
            ) owned
            ORDER BY place, bot_id
            "#,
            owner,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter_map(|row| {
                let runs_on = match (row.place, row.provider.as_deref().and_then(Vendor::parse)) {
                    (0, _) => OwnerBotRuns::Subscription,
                    (1, Some(vendor)) => OwnerBotRuns::OwnKey(vendor),
                    (2, Some(vendor)) => OwnerBotRuns::TeamKey(vendor, row.team_id?),
                    _ => return None,
                };
                Some(OwnerBot {
                    bot: row.bot_id,
                    model: row.model,
                    runs_on,
                })
            })
            .collect())
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
            SET machine_id = $3, slot_wait_since = NULL, slot_wait_machines = NULL,
                slot_wait_reason = NULL
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
            SET machine_id = NULL, slot_wait_since = NULL, slot_wait_machines = NULL,
                slot_wait_reason = NULL
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
        reason: Option<&str>,
        take_within: Duration,
    ) -> Result<bool, Self::Err> {
        // The wait keeps when it began across looks; the slot is let go of
        // meanwhile, so a Step that waits blocks no other.
        let late = sqlx::query_scalar!(
            r#"
            UPDATE factory_step
            SET machine_id = NULL,
                slot_wait_since = COALESCE(slot_wait_since, now()),
                slot_wait_machines = $3,
                slot_wait_reason = $4
            WHERE run_id = $1 AND seq = $2 AND ended_at IS NULL
            RETURNING now() - started_at > make_interval(secs => $5) AS "late!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            machines,
            reason,
            take_within.as_secs_f64(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(late.unwrap_or(false))
    }
}
