//! Where a Step's agent session runs: the Machine (harness) its Agent's bot
//! is bound to. An Agent fits a Step while its Machine has every tag the
//! Agent asks for; a bot on no Machine has no tags. The Step's slot follows
//! the Agent that opens its session: before a session opens, the Step takes
//! a slot on that Agent's Machine, moving any slot it held, and waits while
//! the Machine is full. Before a Step claims its first session, it is
//! placed on the first fitting Agent whose Machine has a free slot.

use std::time::Duration;

use agent_session::domain::routines::RoutineSessions;
use factory_records::domain::evaluation::EvaluationRepository;
use factory_records::domain::machines::{MachineRepository, SlotTake, StepMachine};
use factory_records::domain::models::AgentName;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::session_usage::SessionUsageRepository;
use uuid::Uuid;

use super::{SessionAgent, SessionAgentError};
use crate::domain::agents::{Agent, AgentDefinition};
use crate::domain::models::AgentTask;

/// How long a Step waiting for a slot waits between two looks.
const SLOT_LOOK: Duration = Duration::from_secs(10);

/// How long after it started a Step may still take a slot: short of the two
/// hours its work has across its attempts (the workflow's `STEP_TOTAL`), so
/// a Step that takes one has at least half an hour left to work. Past it a
/// Step that holds no slot fails as one whose Machines stayed full.
pub(crate) const TAKE_SLOT_WITHIN: Duration = Duration::from_secs(90 * 60);

/// Where an Agent's sessions run, for a Step.
enum Fit {
    /// Its Machine lacks a tag it asks for, or it asks for tags and runs on
    /// no Machine.
    Unfit,
    /// It runs on no Machine and asks for no tag: no slot to hold.
    Anywhere,
    /// It runs on this Machine, which has its tags.
    On(StepMachine),
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

/// What the Run's owner is shown when no Machine has the tags the Agents of
/// `listed` ask for.
pub(super) fn no_machine(agents: &AgentDefinition, listed: &[AgentName]) -> String {
    let mut asked: Vec<String> = Vec::new();
    for agent in listed.iter().filter_map(|name| agents.agent(name)) {
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
    /// Where the sessions of the Agent `name` run, for a Step.
    async fn fit(
        &self,
        agents: &AgentDefinition,
        name: &AgentName,
    ) -> Result<Fit, SessionAgentError> {
        let agent = Self::agent(agents, name)?;
        let machine = self
            .repo
            .bot_machine(agent.bot.as_uuid())
            .await
            .map_err(Self::records)?;
        if !has_tags(agent, machine.as_ref()) {
            return Ok(Fit::Unfit);
        }
        Ok(machine.map_or(Fit::Anywhere, Fit::On))
    }

    /// The first Agent of `listed` after `after` (from the first when
    /// `after` is none or not listed) whose Machine has the tags it asks
    /// for: the one a Step passes to when its Agent cannot take it.
    pub(super) async fn next_fitting(
        &self,
        agents: &AgentDefinition,
        listed: &[AgentName],
        after: Option<&AgentName>,
    ) -> Result<Option<AgentName>, SessionAgentError> {
        let from = after
            .and_then(|after| listed.iter().position(|name| name == after))
            .map_or(0, |place| place + 1);
        for name in listed.iter().skip(from) {
            if !matches!(self.fit(agents, name).await?, Fit::Unfit) {
                return Ok(Some(name.clone()));
            }
        }
        Ok(None)
    }

    /// Record that the Step `task` waits on the full Machines `full` and
    /// wait one look; fail for a retry when the attempt has no time left for
    /// one.
    async fn wait_for_slot(
        &self,
        task: &AgentTask,
        full: &[(Uuid, String)],
        deadline: tokio::time::Instant,
    ) -> Result<(), SessionAgentError> {
        let names: Vec<String> = full.iter().map(|(_, name)| name.clone()).collect();
        self.repo
            .wait_for_machine_slot(task.key, &names)
            .await
            .map_err(Self::records)?;
        if deadline.saturating_duration_since(tokio::time::Instant::now()) < SLOT_LOOK {
            return Err(SessionAgentError::WaitingForSlot(names));
        }
        tracing::info!(
            machines = ?names,
            "every machine a factory step may run on is full: it waits for a slot"
        );
        tokio::time::sleep(SLOT_LOOK).await;
        Ok(())
    }

    /// The Agent of `listed` whose session the Step `task` claims first: the
    /// first whose Machine has the tags it asks for and a slot the Step now
    /// holds, or that runs on no Machine and asks for no tag. An Agent its
    /// owner's quota refused lately, with a fitting Agent after it, is
    /// claimed with no slot booked: the session's start passes it over and
    /// the next fitting Agent holds the slot as it opens. A Step that
    /// already claimed a session goes on in it, so it is not placed again.
    ///
    /// While every fitting Machine is full, the Step records that it waits
    /// on them and looks again every [`SLOT_LOOK`]; with no time left before
    /// `deadline`, it fails for a retry ([`SessionAgentError::WaitingForSlot`]),
    /// and past [`TAKE_SLOT_WITHIN`] after it started, for good
    /// ([`SessionAgentError::SlotTimeRanOut`]). With no fitting Agent at
    /// all, it fails at once ([`SessionAgentError::NoMachine`]).
    pub(super) async fn place(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        listed: &[AgentName],
        deadline: tokio::time::Instant,
    ) -> Result<AgentName, SessionAgentError> {
        let claimed = self
            .repo
            .claimed_session(task.key)
            .await
            .map_err(Self::records)?;
        if claimed.is_some() {
            return Ok(listed[0].clone());
        }

        loop {
            let mut fitting = Vec::new();
            for name in listed {
                match self.fit(agents, name).await? {
                    Fit::Unfit => {}
                    fit => fitting.push((name, fit)),
                }
            }
            if fitting.is_empty() {
                return Err(SessionAgentError::NoMachine(no_machine(agents, listed)));
            }

            let last = fitting.len() - 1;
            let mut full = Vec::new();
            for (place, (name, fit)) in fitting.into_iter().enumerate() {
                // The session's start passes over an Agent its owner's quota
                // refused lately while a fitting Agent follows it, recording
                // why: the Step claims it, books no slot for it, and the
                // Agent that opens the session holds one (`hold_slot`).
                if place < last && self.quota.refused_lately(&task.owner, name) {
                    return Ok(name.clone());
                }
                let machine = match fit {
                    Fit::Unfit => continue,
                    Fit::Anywhere => {
                        self.repo
                            .release_machine_slot(task.key)
                            .await
                            .map_err(Self::records)?;
                        return Ok(name.clone());
                    }
                    Fit::On(machine) => machine,
                };
                match self.take(task, &machine).await? {
                    SlotTake::Taken => return Ok(name.clone()),
                    SlotTake::Full => note_full(&mut full, machine),
                    SlotTake::TooLate => {
                        note_full(&mut full, machine);
                        return Err(SessionAgentError::SlotTimeRanOut(
                            full.into_iter().map(|(_, name)| name).collect(),
                        ));
                    }
                }
            }
            self.wait_for_slot(task, &full, deadline).await?;
        }
    }

    /// Hold a slot for the Step `task` on the Machine of the Agent `name`,
    /// whose session is about to open: the slot follows the Agent that
    /// opens it, so one the Step held on another Machine moves here, and a
    /// Step whose Agent runs on no Machine lets go of any. While the Machine
    /// is full, the Step waits as placement does.
    pub(super) async fn hold_slot(
        &self,
        agents: &AgentDefinition,
        task: &AgentTask,
        name: &AgentName,
        deadline: tokio::time::Instant,
    ) -> Result<(), SessionAgentError> {
        let machine = match self.fit(agents, name).await? {
            Fit::Unfit => {
                return Err(SessionAgentError::NoMachine(no_machine(
                    agents,
                    std::slice::from_ref(name),
                )));
            }
            Fit::Anywhere => {
                return self
                    .repo
                    .release_machine_slot(task.key)
                    .await
                    .map_err(Self::records);
            }
            Fit::On(machine) => machine,
        };
        loop {
            match self.take(task, &machine).await? {
                SlotTake::Taken => return Ok(()),
                SlotTake::TooLate => {
                    return Err(SessionAgentError::SlotTimeRanOut(vec![machine.name]));
                }
                SlotTake::Full => {
                    self.wait_for_slot(task, &[(machine.id, machine.name.clone())], deadline)
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
