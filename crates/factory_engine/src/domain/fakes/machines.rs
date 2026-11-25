//! The in-memory stand-in for the Machines Steps are placed on.

use factory_records::domain::machines::{MachineRepository, StepMachine};
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

    /// As the records keep it: the Steps that hold a slot and have not
    /// ended count, this one left out.
    async fn take_machine_slot(&self, key: StepKey, machine: Uuid) -> Result<bool, FakeError> {
        let slots = self
            .machines
            .lock()
            .expect("machines")
            .values()
            .find(|known| known.id == machine)
            .and_then(|known| known.step_slots);
        let mut held = self.machine_slots.lock().expect("slots");
        if let Some(slots) = slots {
            let steps = self.steps.lock().expect("steps");
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
                return Ok(false);
            }
        }
        held.insert(key, machine);
        self.slot_waits.lock().expect("waits").remove(&key);
        Ok(true)
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
