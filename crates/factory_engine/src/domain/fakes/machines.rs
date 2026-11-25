//! The in-memory stand-in for the Machines Steps are placed on.

use std::time::Duration;

use chrono::Utc;
use factory_records::domain::machines::{MachineRepository, SlotTake, StepMachine};
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

    /// As the records keep it: a Step keeps the slot it holds; one that
    /// started more than `take_within` ago takes none; the Steps that hold
    /// a slot and have not ended count, this one left out.
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
        let started = steps
            .iter()
            .find(|(run, step)| *run == key.run_id && step.seq == key.seq)
            .map(|(_, step)| step.started_at);
        let late = started.is_some_and(|started| {
            (Utc::now() - started).to_std().unwrap_or_default() > take_within
        });
        if late {
            return Ok(SlotTake::TooLate);
        }
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
                return Ok(SlotTake::Full);
            }
        }
        held.insert(key, machine);
        self.slot_waits.lock().expect("waits").remove(&key);
        Ok(SlotTake::Taken)
    }

    async fn release_machine_slot(&self, key: StepKey) -> Result<(), FakeError> {
        self.machine_slots.lock().expect("slots").remove(&key);
        self.slot_waits.lock().expect("waits").remove(&key);
        Ok(())
    }

    async fn wait_for_machine_slot(
        &self,
        key: StepKey,
        machines: &[String],
    ) -> Result<(), FakeError> {
        self.slot_waits
            .lock()
            .expect("waits")
            .insert(key, machines.to_vec());
        Ok(())
    }
}
