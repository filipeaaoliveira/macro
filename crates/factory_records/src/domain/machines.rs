//! Where the factory places an agent Step: the Machine (harness) its Agent's
//! bot is bound to, how many Steps that Machine runs at once (its slots) and
//! its tags. A Step takes a slot on its Machine before its session opens and
//! holds it until it ends; while every Machine its Agents may run on is
//! full, it waits, and its Run says so.

#[cfg(test)]
mod test;

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
    /// Whose provider login its sessions run on.
    pub login: MachineLogin,
}

/// Whose provider login a Machine's agent sessions run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineLogin {
    /// A macrod machine a person paired: its provider CLI runs on the login
    /// of this person, its owner, on their own computer.
    Owner(String),
    /// A Macro computer with no provider sign-in of its own: its sessions
    /// call their model through Macro's gateway, on Macro credits.
    MacroCredits,
    /// A Macro computer signed in to a provider through Macro.
    HostedSignIn,
}

/// A bot a Run's owner may run an agent Step on, for an Agent that names a
/// vendor rather than a bot (`MachineRepository::owner_bots`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerBot {
    /// The bot.
    pub bot: Uuid,
    /// Its default model, which says the vendor of a Machine's CLI.
    pub model: String,
    /// What it runs on.
    pub runs_on: OwnerBotRuns,
}

/// What an [`OwnerBot`] runs on, in the order a Step takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OwnerBotRuns {
    /// A Machine the owner paired: their own provider CLI login.
    Subscription,
    /// An agent of the owner's own with its own API key, of this provider.
    OwnKey(Vendor),
    /// An agent of the team `team` the owner is a member of, with its own
    /// API key of this provider.
    TeamKey(Vendor, Uuid),
}

/// A model vendor whose official CLI or API an agent Step runs on: an
/// Agent may name one instead of a bot, and an agent's own API key is for
/// one (`agent_api_keys.provider`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    /// Anthropic: Claude Code, or an Anthropic API key.
    Anthropic,
    /// OpenAI: Codex, or an OpenAI API key.
    #[serde(rename = "openai")]
    OpenAi,
}

impl Vendor {
    /// The vendor as `agent_api_keys.provider` names it.
    pub fn parse(provider: &str) -> Option<Self> {
        match provider {
            "anthropic" => Some(Self::Anthropic),
            "openai" => Some(Self::OpenAi),
            _ => None,
        }
    }

    /// The vendor of the model `model`, as an agent's configuration names
    /// it: a routed `provider/model` id by its provider, else a model's own
    /// id by its family (`claude-…` is Anthropic's, `gpt-…` OpenAI's). None
    /// for a model of neither, or one written as no known family.
    pub fn of_model(model: &str) -> Option<Self> {
        let model = model.trim().to_ascii_lowercase();
        if let Some((provider, _)) = model.split_once('/') {
            return Self::parse(provider);
        }
        let anthropic = ["claude", "opus", "sonnet", "haiku", "fable"];
        let openai = ["gpt", "codex", "o3", "o4"];
        if anthropic.iter().any(|family| model.starts_with(family)) {
            Some(Self::Anthropic)
        } else if openai.iter().any(|family| model.starts_with(family)) {
            Some(Self::OpenAi)
        } else {
            None
        }
    }

    /// The vendor's name as people know it.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
        }
    }
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

    /// The bots `owner`'s agent Steps may run on for an Agent that names a
    /// vendor: each coding agent bound to a macrod machine `owner` paired
    /// (never a Macro computer, nor a machine shared with them) that is
    /// `owner`'s own agent or one of a team they are a member of, then each
    /// in-memory agent of `owner`'s own with its own API key, then each of a
    /// team `owner` is a member of. Each kind in the order its bots were
    /// made.
    fn owner_bots(
        &self,
        owner: &str,
    ) -> impl Future<Output = Result<Vec<OwnerBot>, Self::Err>> + Send;

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
    /// `machines`, by name, or, with `reason`, for a Machine or an API key
    /// it may run on at all, keeping when it first started to wait, and let
    /// go of any slot it holds meanwhile. Answers whether the Step started
    /// more than `take_within` ago, so it should stop waiting.
    fn wait_for_machine_slot(
        &self,
        key: StepKey,
        machines: &[String],
        reason: Option<&str>,
        take_within: Duration,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;
}
