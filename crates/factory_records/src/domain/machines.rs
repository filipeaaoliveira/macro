//! Where the factory places an agent Step: the Machine (harness) its Agent's
//! bot is bound to, how many Steps that Machine runs at once (its slots) and
//! its tags. A Step takes a slot on its Machine before its session opens and
//! holds it until it ends; while every Machine its Agents may run on is
//! full, it waits, and its Run says so.

use std::future::Future;
use std::time::Duration;

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

/// How long after it started a Step that has not ended still holds its
/// slot: past the two hours its work has across its attempts (the engine
/// workflow's `STEP_TOTAL`), with a margin. No work of an older Step can
/// still run, so a Step whose Run's workflow is gone without recording its
/// end frees its slot then, rather than hold it for good.
pub const SLOT_HELD_AT_MOST: Duration = Duration::from_secs(125 * 60);

/// What came of asking for a slot on a Machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotTake {
    /// The Step holds a slot on the Machine now.
    Taken,
    /// Every slot is held by another Step.
    Full,
    /// Every slot is held, and the Step started too long ago to wait for
    /// one: too little of its time would be left for its work.
    TooLate,
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
    /// slot is held by another running Step: then [`SlotTake::Full`], or
    /// [`SlotTake::TooLate`] when the Step started more than `take_within`
    /// ago, so it should stop waiting. A free slot is taken however late. A
    /// Step that holds a slot on another Machine moves it here; one that
    /// holds this Machine's keeps it, however late, so a retried attempt
    /// takes no second slot. A Step holds its slot while it and its Run
    /// have not ended, for at most [`SLOT_HELD_AT_MOST`] after it started.
    /// Taking clears the Step's wait. The Machine is read under a lock of
    /// its own, so two Steps never take its last slot at once.
    fn take_machine_slot(
        &self,
        key: StepKey,
        machine: Uuid,
        take_within: Duration,
    ) -> impl Future<Output = Result<SlotTake, Self::Err>> + Send;

    /// Whether the session the Step `key` works in now has opened: then its
    /// Machine is settled, and the Step takes no slot for it.
    fn session_opened(&self, key: StepKey) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Let go of the Step `key`'s slot, if it holds one, and of its wait:
    /// its session runs on no Machine.
    fn release_machine_slot(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the Step `key` waits for a slot on the full Machines
    /// `machines`, by name, keeping when it first started to wait.
    fn wait_for_machine_slot(
        &self,
        key: StepKey,
        machines: &[String],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
