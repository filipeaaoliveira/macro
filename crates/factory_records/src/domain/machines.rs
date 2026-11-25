//! Where the factory places an agent Step: the Machine (harness) its Agent's
//! bot is bound to, how many Steps that Machine runs at once (its slots) and
//! its tags. A Step takes a slot on its Machine before its session opens and
//! holds it until it ends; while every Machine its Agents may run on is
//! full, it waits, and its Run says so.

use std::future::Future;

use uuid::Uuid;

use crate::domain::models::StepKey;

/// A Machine a Step's agent session may run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepMachine {
    /// The harness id.
    pub id: Uuid,
    /// Its display name.
    pub name: String,
    /// How many factory Steps it runs at once; none for no limit.
    pub step_slots: Option<u16>,
    /// Its tags, lower case and sorted.
    pub tags: Vec<String>,
}

/// Reads Machines and keeps which Step holds a slot on which.
pub trait MachineRepository: Send + Sync + 'static {
    /// The error type returned by reads and writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The Machine the agent `bot` is bound to now; none for a bot bound to
    /// no Machine (a runtime Macro runs, or a macrod agent left unbound) or
    /// to one since removed.
    fn bot_machine(
        &self,
        bot: Uuid,
    ) -> impl Future<Output = Result<Option<StepMachine>, Self::Err>> + Send;

    /// Take a slot on the Machine `machine` for the Step `key`, unless every
    /// slot is held by another Step that has not ended: whether the Step
    /// holds one now. A Step that holds a slot on another Machine moves it
    /// here; one that holds this Machine's keeps it, so a retried attempt
    /// takes no second slot. Taking clears the Step's wait. Machines read
    /// under one lock each, so two Steps never take its last slot at once.
    fn take_machine_slot(
        &self,
        key: StepKey,
        machine: Uuid,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Record that the Step `key` waits for a slot on the full Machines
    /// `machines`, by name, keeping when it first started to wait.
    fn wait_for_machine_slot(
        &self,
        key: StepKey,
        machines: &[String],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
