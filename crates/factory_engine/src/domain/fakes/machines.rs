//! The in-memory stand-in for the Machines Steps are placed on.

use std::time::Duration;

use chrono::Utc;
use factory_records::domain::machines::{MachineRepository, OwnerBot, SlotTake, StepMachine};
use factory_records::domain::models::StepKey;
use uuid::Uuid;

use super::{FakeError, FakeRepo};

impl MachineRepository for FakeRepo {
    type Err = FakeError;

    async fn bot_machine(&self, bot: Uuid) -> Result<Option<StepMachine>, FakeError> {
        if self.fails("bot_machine") {
            return Err(FakeError);
        }
        Ok(self.machines.lock().expect("machines").get(&bot).cloned())
    }

    async fn owner_bots(&self, owner: &str) -> Result<Vec<OwnerBot>, FakeError> {
        if self.fails("owner_bots") {
            return Err(FakeError);
        }
        Ok(self
            .owner_bots
            .lock()
            .expect("owner bots")
            .get(owner)
            .cloned()
            .unwrap_or_default())
    }

    /// As the records keep it: a Step keeps the slot it holds; the Steps
    /// that hold a slot and have not ended count, this one left out; a full
    /// Machine is `TooLate` for a Step that started more than
    /// `take_within` ago.
    async fn take_machine_slot(
        &self,
        key: StepKey,
        machine: Uuid,
        take_within: Duration,
    ) -> Result<SlotTake, FakeError> {
        let mut held = self.machine_slots.lock().expect("slots");
        if held.get(&key) == Some(&machine) {
            return Ok(SlotTake::Taken);
        }
        let steps = self.steps.lock().expect("steps");
        let slots = self
            .machines
            .lock()
            .expect("machines")
            .values()
            .find(|known| known.id == machine)
            .and_then(|known| known.step_slots);
        if let Some(slots) = slots {
            let running = held
                .iter()
                .filter(|(holder, on)| **on == machine && **holder != key)
                .filter(|(holder, _)| {
                    steps.iter().any(|(run, step)| {
                        *run == holder.run_id && step.seq == holder.seq && step.end.is_none()
                    })
                })
                .count();
            if running >= usize::from(slots) {
                let late = steps
                    .iter()
                    .find(|(run, step)| *run == key.run_id && step.seq == key.seq)
                    .is_some_and(|(_, step)| {
                        (Utc::now() - step.started_at).to_std().unwrap_or_default() > take_within
                    });
                return Ok(if late {
                    SlotTake::TooLate
                } else {
                    SlotTake::Full
                });
            }
        }
        held.insert(key, machine);
        self.slot_waits.lock().expect("waits").remove(&key);
        self.slot_wait_reasons.lock().expect("reasons").remove(&key);
        Ok(SlotTake::Taken)
    }

    /// Whether the Step's current session was seen open.
    async fn session_opened(&self, key: StepKey) -> Result<bool, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        Ok(sessions
            .iter()
            .find(|claimed| claimed.key == key)
            .is_some_and(|claimed| claimed.opened))
    }

    async fn release_machine_slot(&self, key: StepKey) -> Result<(), FakeError> {
        self.machine_slots.lock().expect("slots").remove(&key);
        self.slot_waits.lock().expect("waits").remove(&key);
        self.slot_wait_reasons.lock().expect("reasons").remove(&key);
        Ok(())
    }

    async fn wait_for_machine_slot(
        &self,
        key: StepKey,
        machines: &[String],
        reason: Option<&str>,
    ) -> Result<(), FakeError> {
        self.slot_waits
            .lock()
            .expect("waits")
            .insert(key, machines.to_vec());
        let mut reasons = self.slot_wait_reasons.lock().expect("reasons");
        match reason {
            Some(reason) => reasons.insert(key, reason.to_string()),
            None => reasons.remove(&key),
        };
        Ok(())
    }
}
