//! Where a Step's agent session runs: the Machine (harness) its Agent's bot
//! is bound to. Before a Step claims its session, it takes the first Agent
//! of its Role, in order, whose Machine has every tag the Agent asks for
//! and a free slot. While each such Machine is full the Step waits, and its
//! Run says on which Machines; when none has the tags, it fails at once.

use std::time::Duration;

use agent_session::domain::routines::RoutineSessions;
use factory_records::domain::evaluation::EvaluationRepository;
use factory_records::domain::machines::{MachineRepository, StepMachine};
use factory_records::domain::models::AgentName;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::session_usage::SessionUsageRepository;

use super::{SessionAgent, SessionAgentError};
use crate::domain::agents::{Agent, AgentDefinition};
use crate::domain::models::AgentTask;

/// How long a Step waiting for a slot waits between two looks.
const SLOT_LOOK: Duration = Duration::from_secs(10);

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
fn no_machine(agents: &AgentDefinition, listed: &[AgentName]) -> String {
    let asked = listed
        .iter()
        .filter_map(|name| agents.agent(name))
        .map(|agent| agent.machine_tags.join(", "))
        .collect::<Vec<_>>()
        .join("; or ");
    format!(
        "No machine the step's agents run on has the tags they ask for ({asked}): \
         give a machine those tags in Settings, Machines, or bind an agent to one that has them."
    )
}

impl<
    S: RoutineSessions,
    R: RunRepository + EvaluationRepository + SessionUsageRepository + MachineRepository,
> SessionAgent<S, R>
{
    /// The Agent of `listed` whose session the Step `task` claims: the first
    /// whose Machine has the tags it asks for and a slot the Step now holds,
    /// or that runs on no Machine and asks for no tag. A Step that already
    /// claimed a session goes on in it, so it is not placed again.
    ///
    /// While every matching Machine is full, the Step records that it waits
    /// on them and looks again every [`SLOT_LOOK`]; with no time left before
    /// `deadline`, it fails for a retry ([`SessionAgentError::WaitingForSlot`]).
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
            let mut full = Vec::new();
            for name in listed {
                let agent = Self::agent(agents, name)?;
                let machine = self
                    .repo
                    .bot_machine(agent.bot.as_uuid())
                    .await
                    .map_err(Self::records)?;
                if !has_tags(agent, machine.as_ref()) {
                    continue;
                }
                let Some(machine) = machine else {
                    return Ok(name.clone());
                };
                let taken = self
                    .repo
                    .take_machine_slot(task.key, machine.id)
                    .await
                    .map_err(Self::records)?;
                if taken {
                    return Ok(name.clone());
                }
                if !full.contains(&machine.name) {
                    full.push(machine.name);
                }
            }

            if full.is_empty() {
                return Err(SessionAgentError::NoMachine(no_machine(agents, listed)));
            }
            self.repo
                .wait_for_machine_slot(task.key, &full)
                .await
                .map_err(Self::records)?;
            if deadline.saturating_duration_since(tokio::time::Instant::now()) < SLOT_LOOK {
                return Err(SessionAgentError::WaitingForSlot(full));
            }
            tracing::info!(
                machines = ?full,
                "every machine a factory step may run on is full: it waits for a slot"
            );
            tokio::time::sleep(SLOT_LOOK).await;
        }
    }
}
