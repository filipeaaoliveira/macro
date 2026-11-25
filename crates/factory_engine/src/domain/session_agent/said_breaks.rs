//! The break lines an agent wrote in its reply rather than its hand back.
//! A write or fix Step's checks round reads breaks only from what the Step
//! handed back ([`crate::domain::breaks`]), and agents often put them in
//! the reply that ends their turn instead: each readable one the hand back
//! lacks is added to it, so no break is lost. A fix's answer of its
//! Blockers ([`crate::domain::review_rounds::fix_answers`]) is taken from
//! the reply the same way, when the hand back holds none.

use agent_runtime_protocol::domain::action::AgentActionId;
use agent_session::domain::routines::{RoutineSessionAction, RoutineSessions};
use factory_records::domain::evaluation::EvaluationRepository;
use factory_records::domain::machines::MachineRepository;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::session_usage::SessionUsageRepository;

use super::SessionAgent;
use crate::domain::breaks::{Reported, read_line, reported};
use crate::domain::models::AgentAnswer;
use crate::domain::review_rounds::json_blocks;

/// What heads the break lines added from the agent's reply.
pub(super) const FROM_REPLY: &str = "Breaks the agent wrote in its reply:";

impl<
    S: RoutineSessions,
    R: RunRepository + EvaluationRepository + SessionUsageRepository + MachineRepository,
> SessionAgent<S, R>
{
    /// `answer`, with each readable break line the agent wrote in its
    /// replies to `turns` (prompts of `action`'s session) whose test the
    /// hand back names no break for, added at its end. A reply that cannot
    /// be read leaves the answer as it is: the hand back still stands.
    pub(super) async fn with_said_breaks(
        &self,
        mut answer: AgentAnswer,
        action: &RoutineSessionAction,
        turns: &[AgentActionId],
    ) -> AgentAnswer {
        let mut read: Vec<AgentActionId> = Vec::with_capacity(turns.len());
        let mut said = String::new();
        for &turn in turns {
            if read.contains(&turn) {
                continue;
            }
            read.push(turn);
            let turn = RoutineSessionAction {
                action_id: turn,
                ..action.clone()
            };
            match self.sessions.reply(turn).await {
                Ok(Some(reply)) => {
                    said.push_str(&reply);
                    said.push('\n');
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        "a factory step's agent's reply was not read for breaks"
                    );
                }
            }
        }
        let added = missing_breaks(&answer.text, &said);
        if !added.is_empty() {
            answer.text = format!(
                "{}\n\n{FROM_REPLY}\n{}",
                answer.text.trim_end(),
                added.join("\n")
            );
        }
        if let Some(block) = missing_fix_answer(&answer.text, &said) {
            answer.text = format!("{}\n\n{block}", answer.text.trim_end());
        }
        answer
    }
}

/// The last fenced `json` block of `said` that answers a fix's Blockers,
/// when `handed` holds none: fenced again, to be added at the end of
/// `handed`, where the fix's answer is read.
pub(super) fn missing_fix_answer(handed: &str, said: &str) -> Option<String> {
    const KEY: &str = "blockers";
    if json_blocks(handed).iter().any(|block| block.names(KEY)) {
        return None;
    }
    let block = json_blocks(said)
        .into_iter()
        .rev()
        .find(|block| block.names(KEY))?;
    Some(format!("```json\n{}\n```", block.value))
}

/// The readable break lines of `said` whose test `handed` names no break
/// for, each once, the latest line for a test kept.
pub(super) fn missing_breaks(handed: &str, said: &str) -> Vec<String> {
    let named: Vec<String> = reported([handed])
        .iter()
        .filter(|kept| matches!(kept, Reported::Break(_)))
        .map(|kept| kept.test().to_string())
        .collect();
    let mut added: Vec<(String, String)> = Vec::new();
    for line in said.lines() {
        let Some(Reported::Break(read)) = read_line(line) else {
            continue;
        };
        if named.contains(&read.test) {
            continue;
        }
        let line = line.trim().to_string();
        match added.iter_mut().find(|(test, _)| *test == read.test) {
            Some(kept) => kept.1 = line,
            None => added.push((read.test, line)),
        }
    }
    added.into_iter().map(|(_, line)| line).collect()
}
