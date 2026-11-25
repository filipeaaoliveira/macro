//! Where a Step's agent session runs: the Machine (harness) its Agent's bot
//! is bound to. An Agent that names a vendor rather than a bot runs on the
//! Run's owner's own bots for it ([`SessionAgent::on_bots`]): their coding
//! agents on macrod machines they paired, then their agents with an API key
//! of the vendor's, then their teams'. A Machine's provider login is spent
//! only on its owner's Runs: an Agent whose Machine someone else paired, or
//! a Macro computer signed in to a provider through Macro, does not fit the
//! Step. An Agent fits a Step while its Machine has every tag the Agent asks
//! for, too; a bot on no Machine has no tags. The Step's slot follows the
//! Agent that opens its session: before a session opens, the Step takes a
//! slot on that Agent's Machine, moving any slot it held, and waits while
//! the Machine is full. Before a Step claims its first session, it is placed
//! on the first fitting Agent whose Machine has a free slot, and waits,
//! saying why, while none may run it for its owner.

use std::collections::HashMap;
use std::time::Duration;

use agent_api_keys::domain::AgentOwner;
use agent_session::domain::routines::RoutineSessions;
use bot_id::BotId;
use factory_records::domain::evaluation::EvaluationRepository;
use factory_records::domain::machines::{
    MachineLogin, MachineRepository, OwnerBot, OwnerBotRuns, SlotTake, StepMachine, Vendor,
};
use factory_records::domain::models::AgentName;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::session_usage::SessionUsageRepository;
use uuid::Uuid;

use super::{AgentBot, SessionAgent, SessionAgentError};
use crate::domain::agents::{Agent, AgentDefinition};
use crate::domain::models::AgentTask;

/// How long a Step waiting for a slot waits between two looks.
const SLOT_LOOK: Duration = Duration::from_secs(10);

/// How long after it started a Step may still take a slot: short of the two
/// hours its work has across its attempts (the workflow's `STEP_TOTAL`), so
/// a Step that takes one has at least half an hour left to work. Past it a
/// Step that holds no slot fails as one whose Machines stayed full, and one
/// with no Machine or key it may run on fails as such.
pub(crate) const TAKE_SLOT_WITHIN: Duration = Duration::from_secs(90 * 60);

/// The longest reason a waiting Step keeps, in characters: the table keeps
/// at most 2000.
const REASON_CHARS: usize = 1900;

/// Where an Agent's sessions run, for a Step.
enum Fit {
    /// Its Machine lacks a tag it asks for, or it asks for tags and runs on
    /// no Machine.
    Unfit,
    /// Its Machine's provider login is not the Run's owner's to spend: the
    /// Machine is someone else's, or a Macro computer signed in through
    /// Macro. It holds what the owner is shown.
    NotTheOwners(String),
    /// It runs on no Machine and asks for no tag: no slot to hold.
    Anywhere,
    /// It runs on this Machine, which has its tags.
    On(StepMachine),
}

/// What came of holding a slot for an Agent whose session is about to open.
pub(super) enum Hold {
    /// The Step holds a slot on its Machine, or needs none.
    Held,
    /// Its Machine's provider login is not the Run's owner's: the session
    /// must not open there. It holds why.
    NotTheOwners(String),
}

/// The Agents of a Step's Role on the bots that run them for its owner,
/// and why each Agent that names a vendor has none.
struct Resolved {
    bots: Vec<AgentBot>,
    unmet: Vec<String>,
}

/// Whether `machine` has every tag `agent` asks for, ignoring case.
fn has_tags(agent: &Agent, machine: Option<&StepMachine>) -> bool {
    agent.machine_tags.iter().all(|asked| {
        machine.is_some_and(|machine| {
            machine
                .tags
                .iter()
                .any(|tag| tag.eq_ignore_ascii_case(asked))
        })
    })
}

/// Why `owner`'s Step may not run on `machine`, the Agent `name`'s: none
/// when it may. A macrod machine runs its provider CLI on the login of the
/// person who paired it, so it runs only that person's Steps, whoever else
/// it is shared with; a Macro computer runs on Macro credits, unless a
/// provider sign-in was made on it through Macro, which no Step runs on.
fn not_the_owners(name: &AgentName, machine: &StepMachine, owner: &str) -> Option<String> {
    match &machine.login {
        MachineLogin::Owner(paired_by) if paired_by == owner => None,
        MachineLogin::MacroCredits => None,
        MachineLogin::Owner(_) => Some(format!(
            "agent {name} runs on machine {}, which is not yours: a machine's provider \
             login runs only its owner's steps",
            machine.name
        )),
        MachineLogin::HostedSignIn => Some(format!(
            "agent {name} runs on Macro computer {}, signed in to a provider through Macro, \
             which runs no factory step",
            machine.name
        )),
    }
}

/// Whether `owned` runs `vendor`'s model: a Machine's agent by its model, an
/// agent with a key by the key's vendor and its model's.
fn serves(owned: &OwnerBot, vendor: Vendor) -> bool {
    let model = Vendor::of_model(&owned.model) == Some(vendor);
    match owned.runs_on {
        OwnerBotRuns::Subscription => model,
        OwnerBotRuns::OwnKey(key) | OwnerBotRuns::TeamKey(key, _) => key == vendor && model,
    }
}

/// What the Run's owner is shown when no Machine has the tags the Agents of
/// `listed` ask for.
pub(super) fn no_machine(agents: &AgentDefinition, listed: &[AgentBot]) -> String {
    let mut asked: Vec<String> = Vec::new();
    for agent in listed.iter().filter_map(|on| agents.agent(&on.agent)) {
        let tags = agent.machine_tags.join(", ");
        if !asked.contains(&tags) {
            asked.push(tags);
        }
    }
    let asked = asked.join("; or ");
    format!(
        "No machine the step's agents run on has the tags they ask for ({asked}): \
         give a machine those tags in Settings, Machines, or bind an agent to one that has them."
    )
}

/// What `vendor`'s Agent `name` is shown when its owner has no Machine or
/// API key for it they may run on: `unrecognised` are the models of their
/// machines' agents that name no vendor, and `unpaid` whether a key of the
/// vendor's was left out as its owner's plan does not include keys.
fn no_vendor_bot(name: &AgentName, vendor: Vendor, unrecognised: &[&str], unpaid: bool) -> String {
    let vendor = vendor.display_name();
    let mut why = format!(
        "agent {name} runs on your own {vendor} machine or API key, and you have none it may run on"
    );
    if !unrecognised.is_empty() {
        why.push_str(&format!(
            ": no machine of yours has an agent with a recognised vendor (its model is {})",
            unrecognised.join(", ")
        ));
    }
    if unpaid {
        why.push_str(": an API key's agent is on a plan that does not include API keys");
    }
    why.push_str(&format!(
        ". Pair a machine of your own whose agent runs {vendor}'s CLI signed in with your login, \
         or give an agent of yours or your team's an {vendor} API key on a paid plan"
    ));
    why
}

/// `reason`, cut to [`REASON_CHARS`] characters.
fn cut(reason: String) -> String {
    match reason.char_indices().nth(REASON_CHARS) {
        Some((end, _)) => format!("{}…", &reason[..end]),
        None => reason,
    }
}

/// Add `machine` to the full Machines a Step waits on, once.
fn note_full(full: &mut Vec<(Uuid, String)>, machine: StepMachine) {
    if !full.iter().any(|(id, _)| *id == machine.id) {
        full.push((machine.id, machine.name));
    }
}

impl<
    S: RoutineSessions,
    R: RunRepository + EvaluationRepository + SessionUsageRepository + MachineRepository,
> SessionAgent<S, R>
{
    /// The Agents `names` on the bots that run them for `task`'s owner, in
    /// order: an Agent that names a bot on it, and one that names a vendor
    /// on each of the owner's bots for it, as `owner_bots` orders them
    /// (their Machines, then their own keys, then their teams'), a key's
    /// only while its owner's plan includes keys. The owner's bots are read
    /// once, when an Agent names a vendor.
    pub(super) async fn on_bots(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        names: &[AgentName],
    ) -> Result<Vec<AgentBot>, SessionAgentError> {
        Ok(self.resolve(agents, task, names).await?.bots)
    }

    /// [`Self::on_bots`], with why each vendor Agent has no bot.
    async fn resolve(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        names: &[AgentName],
    ) -> Result<Resolved, SessionAgentError> {
        let mut owned: Option<Vec<OwnerBot>> = None;
        let mut paid: HashMap<Option<Uuid>, bool> = HashMap::new();
        let mut resolved = Resolved {
            bots: Vec::new(),
            unmet: Vec::new(),
        };
        for name in names {
            let agent = Self::agent(agents, name)?;
            if let Some(bot) = agent.bot {
                resolved.bots.push(AgentBot {
                    agent: name.clone(),
                    bot,
                });
                continue;
            }
            let Some(vendor) = agent.vendor else {
                continue;
            };
            if owned.is_none() {
                owned = Some(
                    self.repo
                        .owner_bots(task.owner.as_ref())
                        .await
                        .map_err(Self::records)?,
                );
            }
            let mut found = false;
            let mut unpaid = false;
            for bot in owned.iter().flatten().filter(|owned| serves(owned, vendor)) {
                let team = match bot.runs_on {
                    OwnerBotRuns::Subscription => None,
                    OwnerBotRuns::OwnKey(_) => Some(None),
                    OwnerBotRuns::TeamKey(_, team) => Some(Some(team)),
                };
                if let Some(team) = team {
                    let allowed = match paid.get(&team) {
                        Some(allowed) => *allowed,
                        None => {
                            let allowed = self.keys_allowed(task, team).await;
                            paid.insert(team, allowed);
                            allowed
                        }
                    };
                    if !allowed {
                        unpaid = true;
                        continue;
                    }
                }
                let on = AgentBot {
                    agent: name.clone(),
                    bot: BotId::new_from_uuid(bot.bot),
                };
                found = true;
                if !resolved.bots.contains(&on) {
                    resolved.bots.push(on);
                }
            }
            if !found {
                let unrecognised: Vec<&str> = owned
                    .iter()
                    .flatten()
                    .filter(|owned| {
                        owned.runs_on == OwnerBotRuns::Subscription
                            && Vendor::of_model(&owned.model).is_none()
                    })
                    .map(|owned| owned.model.as_str())
                    .collect();
                resolved
                    .unmet
                    .push(no_vendor_bot(name, vendor, &unrecognised, unpaid));
            }
        }
        Ok(resolved)
    }

    /// Whether the plan of the agent's owner (`team`, else the Step's owner)
    /// lets an agent's own API key be spent: no when no plans are read, or
    /// they cannot be, so a Step never runs on a key whose turn would fall
    /// back to Macro credits.
    async fn keys_allowed(&self, task: &AgentTask, team: Option<Uuid>) -> bool {
        let Some(plans) = &self.key_plans else {
            return false;
        };
        let owner = match team {
            Some(team) => AgentOwner::Team(team),
            None => AgentOwner::User(task.owner.clone()),
        };
        plans
            .0
            .allows_keys(&owner)
            .await
            .inspect_err(|error| {
                tracing::warn!(
                    ?error,
                    "a key owner's plan was not read: its key is not taken"
                );
            })
            .unwrap_or(false)
    }

    /// Where the sessions of `on` run, for the Step `task`.
    async fn fit(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        on: &AgentBot,
    ) -> Result<Fit, SessionAgentError> {
        let agent = Self::agent(agents, &on.agent)?;
        let machine = self
            .repo
            .bot_machine(on.bot.as_uuid())
            .await
            .map_err(Self::records)?;
        if let Some(why) = machine
            .as_ref()
            .and_then(|machine| not_the_owners(&on.agent, machine, task.owner.as_ref()))
        {
            return Ok(Fit::NotTheOwners(why));
        }
        if !has_tags(agent, machine.as_ref()) {
            return Ok(Fit::Unfit);
        }
        Ok(machine.map_or(Fit::Anywhere, Fit::On))
    }

    /// The first of `listed` after `after` (from the first when `after` is
    /// none or not listed) that fits the Step `task`: the one a Step passes
    /// to when its Agent cannot take it.
    pub(super) async fn next_fitting(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        listed: &[AgentBot],
        after: Option<&AgentBot>,
    ) -> Result<Option<AgentBot>, SessionAgentError> {
        let from = after
            .and_then(|after| listed.iter().position(|on| on == after))
            .map_or(0, |place| place + 1);
        for on in listed.iter().skip(from) {
            if matches!(
                self.fit(agents, task, on).await?,
                Fit::On(_) | Fit::Anywhere
            ) {
                return Ok(Some(on.clone()));
            }
        }
        Ok(None)
    }

    /// Record that the Step `task` waits, letting go of any slot it holds,
    /// on the full Machines `full` or, with `reason`, for a Machine or key
    /// it may run on, and wait one look. It fails for a retry when the
    /// attempt has no time left for one, and, waiting with a reason, for
    /// good past [`TAKE_SLOT_WITHIN`] after the Step started; a slot's wait
    /// learns that from its take.
    pub(super) async fn wait_for_slot(
        &self,
        task: &AgentTask,
        full: &[(Uuid, String)],
        reason: Option<&str>,
        deadline: tokio::time::Instant,
    ) -> Result<(), SessionAgentError> {
        let names: Vec<String> = full.iter().map(|(_, name)| name.clone()).collect();
        let late = self
            .repo
            .wait_for_machine_slot(task.key, &names, reason, TAKE_SLOT_WITHIN)
            .await
            .map_err(Self::records)?;
        if let Some(reason) = reason
            && late
        {
            return Err(SessionAgentError::MachineOrKeyTimeRanOut(
                reason.to_string(),
            ));
        }
        if deadline.saturating_duration_since(tokio::time::Instant::now()) < SLOT_LOOK {
            return Err(match reason {
                Some(reason) => SessionAgentError::WaitingForMachineOrKey(reason.to_string()),
                None => SessionAgentError::WaitingForSlot(names),
            });
        }
        tracing::info!(
            machines = ?names,
            waits_for_machine_or_key = reason.is_some(),
            "no machine a factory step may run on has a free slot: it waits"
        );
        tokio::time::sleep(SLOT_LOOK).await;
        Ok(())
    }

    /// The Agent, on its bot, whose session the Step `task` claims first:
    /// see [`Self::first_fitting`]. A Step that already claimed a session
    /// goes on in it, so it is not placed again.
    pub(super) async fn place(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        deadline: tokio::time::Instant,
    ) -> Result<AgentBot, SessionAgentError> {
        let claimed = self
            .repo
            .claimed_session(task.key)
            .await
            .map_err(Self::records)?;
        // A Step that claimed its session goes on in it: `claim_session`
        // hands that session back whatever Agent is named here, so the
        // session's own stands in, and no slot is taken.
        if let Some(claimed) = claimed {
            return Self::agent_bot(agents, &claimed, &task.role);
        }
        self.first_fitting(agents, task, deadline).await
    }

    /// The first Agent of the Step `task`'s Role, on its bot, that fits it
    /// and whose Machine has a slot the Step now holds, or that runs on no
    /// Machine and asks for no tag. An Agent its owner's quota refused
    /// lately, with a fitting Agent after it, is taken with no slot booked:
    /// the session's start passes it over and the next fitting Agent holds
    /// the slot as it opens.
    ///
    /// While every fitting Machine is full, the Step records that it waits
    /// on them and looks again every [`SLOT_LOOK`]; with no time left before
    /// `deadline`, it fails for a retry ([`SessionAgentError::WaitingForSlot`]),
    /// and past [`TAKE_SLOT_WITHIN`] after it started, for good
    /// ([`SessionAgentError::SlotTimeRanOut`]). While no Agent fits because
    /// none has a Machine or key its owner may spend (its Machine is someone
    /// else's, or its owner has none for its vendor), the Step waits the
    /// same way, saying why, reading the owner's Machines and keys again at
    /// each look ([`SessionAgentError::WaitingForMachineOrKey`],
    /// [`SessionAgentError::MachineOrKeyTimeRanOut`]). With no fitting
    /// Agent for want of tags only, it fails at once
    /// ([`SessionAgentError::NoMachine`]).
    pub(super) async fn first_fitting(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        deadline: tokio::time::Instant,
    ) -> Result<AgentBot, SessionAgentError> {
        if agents.for_role(&task.role).is_empty() {
            return Err(SessionAgentError::UnfilledRole(task.role.clone()));
        }
        loop {
            let (names, trial) = self.ordered_names(agents, task).await?;
            let mut resolved = self.resolve(agents, task, &names).await?;
            if trial {
                resolved.bots.truncate(1);
            }
            let mut fitting = Vec::new();
            let mut not_the_owners = Vec::new();
            for on in &resolved.bots {
                match self.fit(agents, task, on).await? {
                    Fit::Unfit => {}
                    Fit::NotTheOwners(why) => not_the_owners.push(why),
                    fit => fitting.push((on, fit)),
                }
            }
            if fitting.is_empty() {
                not_the_owners.append(&mut resolved.unmet);
                if not_the_owners.is_empty() {
                    return Err(SessionAgentError::NoMachine(no_machine(
                        agents,
                        &resolved.bots,
                    )));
                }
                let reason = cut(format!(
                    "No machine or API key this step may run on for you: {}.",
                    not_the_owners.join("; ")
                ));
                self.wait_for_slot(task, &[], Some(&reason), deadline)
                    .await?;
                continue;
            }

            let last = fitting.len() - 1;
            let mut full = Vec::new();
            let mut too_late = false;
            for (place, (on, fit)) in fitting.into_iter().enumerate() {
                // The session's start passes over an Agent its owner's quota
                // refused lately while a fitting Agent follows it, recording
                // why: the Step claims it, books no slot for it, and the
                // Agent that opens the session holds one (`hold_slot`).
                if place < last && self.quota.refused_lately(&task.owner, on) {
                    return Ok(on.clone());
                }
                let machine = match fit {
                    Fit::Unfit | Fit::NotTheOwners(_) => continue,
                    Fit::Anywhere => {
                        self.repo
                            .release_machine_slot(task.key)
                            .await
                            .map_err(Self::records)?;
                        return Ok(on.clone());
                    }
                    Fit::On(machine) => machine,
                };
                match self.take(task, &machine).await? {
                    SlotTake::Taken => return Ok(on.clone()),
                    SlotTake::Full => note_full(&mut full, machine),
                    SlotTake::TooLate => {
                        too_late = true;
                        note_full(&mut full, machine);
                    }
                }
            }
            // Every fitting Machine is full. A Step too late to take a slot
            // stops waiting; any other waits, holding no slot meanwhile, so
            // a slot it held from an earlier attempt blocks no other Step.
            if too_late {
                return Err(SessionAgentError::SlotTimeRanOut(
                    full.into_iter().map(|(_, name)| name).collect(),
                ));
            }
            self.wait_for_slot(task, &full, None, deadline).await?;
        }
    }

    /// Hold a slot for the Step `task` on the Machine of `on`, whose session
    /// is about to open: the slot follows the Agent that opens it, so one
    /// the Step held on another Machine moves here, and a Step whose Agent
    /// runs on no Machine lets go of any. While the Machine is full, the
    /// Step waits as placement does, having let go of the slot it held
    /// elsewhere, so two Steps moving between two full Machines never hold
    /// each other's way. A Machine whose provider login is not the Run's
    /// owner's takes no slot, and the caller hands the Step on or waits
    /// ([`Hold::NotTheOwners`]).
    ///
    /// A Step whose session already opened, as on a retry, keeps the slot
    /// it holds, if any, and takes none: the session stays where it opened,
    /// whatever its Agent's tags or bot say now, and a Step running from
    /// before Machines had slots goes on without one.
    pub(super) async fn hold_slot(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        on: &AgentBot,
        deadline: tokio::time::Instant,
    ) -> Result<Hold, SessionAgentError> {
        let opened = self
            .repo
            .session_opened(task.key)
            .await
            .map_err(Self::records)?;
        if opened {
            return Ok(Hold::Held);
        }
        let machine = match self.fit(agents, task, on).await? {
            Fit::Unfit => {
                return Err(SessionAgentError::NoMachine(no_machine(
                    agents,
                    std::slice::from_ref(on),
                )));
            }
            Fit::NotTheOwners(why) => {
                return Ok(Hold::NotTheOwners(cut(format!(
                    "No machine or API key this step may run on for you: {why}."
                ))));
            }
            Fit::Anywhere => {
                self.repo
                    .release_machine_slot(task.key)
                    .await
                    .map_err(Self::records)?;
                return Ok(Hold::Held);
            }
            Fit::On(machine) => machine,
        };
        loop {
            match self.take(task, &machine).await? {
                SlotTake::Taken => return Ok(Hold::Held),
                SlotTake::TooLate => {
                    return Err(SessionAgentError::SlotTimeRanOut(vec![machine.name]));
                }
                SlotTake::Full => {
                    self.wait_for_slot(task, &[(machine.id, machine.name.clone())], None, deadline)
                        .await?;
                }
            }
        }
    }

    /// Ask for a slot on `machine` for the Step `task`.
    async fn take(
        &self,
        task: &AgentTask,
        machine: &StepMachine,
    ) -> Result<SlotTake, SessionAgentError> {
        self.repo
            .take_machine_slot(task.key, machine.id, TAKE_SLOT_WITHIN)
            .await
            .map_err(Self::records)
    }
}
