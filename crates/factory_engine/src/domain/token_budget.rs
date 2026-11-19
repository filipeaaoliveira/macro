//! A Run's token budget: the most tokens its agent Steps may use.
//!
//! A Run's Orchestrator Definition may set one (`token_budget`), which the
//! Run keeps with its rules when it is created, so a later change to the
//! definition does not change a Run under way. Before each agent Step, the
//! workflow asks the worker to check it ([`BudgetCheck`]): the worker sums
//! what the sessions of the Run's Steps used, from the usage log, as
//! Insights counts it ([`tokens_used`]: input, output, and input read from
//! or written to the prompt cache, all of which the model bills). A Run
//! that used its budget or more keeps why ([`BudgetStop`]) and ends
//! `blocked`, and its owner is told once it has ended ([`budget_notice`]).
//!
//! A usage log that cannot be read does not end the Run: the check is
//! logged and the Step runs. It does so at most [`MAX_UNREAD_CHECKS`] times
//! in a row: the check after those ends the Run, as unread, so a Run whose
//! usage is never read cannot run unchecked for long. A Run with no budget
//! is never checked.

#[cfg(test)]
mod test;

use std::sync::Arc;

use ai_usage::domain::session_usage::{SessionTokens, SessionUsageReader};
use factory_records::domain::models::{BudgetStop, Run, RunId, RunOutcome};
use macro_user_id::user_id::MacroUserIdStr;
use model_notifications::FactoryGateWaitingMetadata;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::gate_owner::GateNotice;

/// How many agent Steps in a row a Run may take while its usage cannot be
/// read: the check after them ends the Run.
pub const MAX_UNREAD_CHECKS: u8 = 3;

/// What the workflow asks before an agent Step of a Run with a budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetCheck {
    /// The Run.
    pub run_id: RunId,
    /// How many checks in a row before this one could not read the usage
    /// log.
    pub unread: u8,
}

/// What a budget check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetVerdict {
    /// The Run used less than its budget, or has none: its Step runs.
    Within,
    /// The usage log could not be read: the Step runs, and the next check
    /// counts this one.
    Unread,
    /// The budget ended the Run, and why is kept: it ends `blocked`.
    Stopped,
}

/// The tokens `tokens` count, as Insights sums them: input, output, and
/// input read from or written to the prompt cache.
pub fn tokens_used(tokens: &[SessionTokens]) -> u64 {
    tokens
        .iter()
        .map(|used| {
            used.input
                .saturating_add(used.output)
                .saturating_add(used.cache_read)
                .saturating_add(used.cache_write)
        })
        .fold(0, u64::saturating_add)
}

/// The budget the Run `run` keeps, if any.
pub fn budget_of(run: &Run) -> Option<u64> {
    run.standing_rules
        .and_then(|rules| rules.definition.token_budget)
}

/// What the check finds of a Run that used `used` tokens of `budget`, or
/// whose usage could not be read (`None`) after `unread` checks in a row
/// that could not either: the stop to keep, if the budget ends it.
pub fn verdict(budget: u64, used: Option<u64>, unread: u8) -> Result<BudgetVerdict, BudgetStop> {
    match used {
        Some(used) if used >= budget => Err(BudgetStop::Spent { used }),
        Some(_) => Ok(BudgetVerdict::Within),
        None if unread.saturating_add(1) >= MAX_UNREAD_CHECKS => Err(BudgetStop::Unread),
        None => Ok(BudgetVerdict::Unread),
    }
}

/// Reads what a Run's agent sessions used from the usage log. The Run API
/// and the worker hold it behind `dyn`, so neither takes another type
/// parameter for it.
#[async_trait::async_trait]
pub trait RunTokens: Send + Sync {
    /// The tokens `sessions`, run as `owner`, used, as [`tokens_used`]
    /// counts them.
    async fn used(
        &self,
        owner: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<u64, rootcause::Report>;
}

#[async_trait::async_trait]
impl<U: SessionUsageReader> RunTokens for U {
    async fn used(
        &self,
        owner: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<u64, rootcause::Report> {
        let tokens = self
            .session_tokens(owner, sessions)
            .await
            .map_err(|error| rootcause::Report::new(error).into_dynamic())?;
        Ok(tokens_used(&tokens))
    }
}

/// The usage log a service reads Runs' tokens from.
#[derive(Clone)]
pub struct UsageLog(pub Arc<dyn RunTokens>);

impl std::fmt::Debug for UsageLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UsageLog")
    }
}

/// A Run's token budget as the Run API shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunTokenBudget {
    /// The most tokens its agent Steps may use, as it kept it.
    pub budget: u64,
    /// The tokens they used so far, as Insights counts them; none when
    /// the usage log could not be read, or the Run was listed rather than
    /// read on its own.
    pub used: Option<u64>,
    /// Why the budget ended the Run, when it did.
    pub stop: Option<BudgetStop>,
}

/// The id of the notification that tells the owner of the Run `run_id`
/// that its budget ended it: one per Run, so a retry is the same one.
pub fn notice_id(run_id: RunId) -> Uuid {
    let name = format!("{run_id}:token-budget");
    Uuid::new_v5(&NOTICE_NAMESPACE, name.as_bytes())
}

/// The namespace the budget notice's id is made in.
const NOTICE_NAMESPACE: Uuid = Uuid::from_u128(0x6661_6374_6f72_792d_6275_6467_6574_2d6e);

/// What the notice tells the owner of a Run `stop` ended at `budget`.
pub fn stop_text(budget: u64, stop: BudgetStop) -> String {
    match stop {
        BudgetStop::Spent { used } => format!(
            "This run stopped at its token budget: its agents used {used} tokens of {budget}. Start it again under a larger budget to go on."
        ),
        BudgetStop::Unread => format!(
            "This run stopped: its token usage could not be read for {MAX_UNREAD_CHECKS} agent steps in a row, so its budget of {budget} tokens could not be checked."
        ),
    }
}

/// The notice that tells the owner of `run` its budget ended it, through
/// the Gate notice's path: once the Run ended `blocked` with a budget stop,
/// unless its Orchestrator Definition is quiet. It is pushed to their
/// phone, as a Gate that waits on them is: the Run waits on them to start
/// it again.
pub fn budget_notice(run: &Run) -> Option<GateNotice> {
    let stop = run.budget_stop?;
    let budget = budget_of(run)?;
    let rules = run.standing_rules?;
    let ended_blocked = run
        .end
        .as_ref()
        .is_some_and(|end| end.outcome == RunOutcome::Blocked);
    if rules.definition.quiet_gates || !ended_blocked {
        return None;
    }
    Some(GateNotice {
        notification_id: notice_id(run.id),
        owner: run.user_id.clone(),
        task_id: run.task_id,
        metadata: FactoryGateWaitingMetadata {
            run_id: run.id.as_uuid(),
            // No Step has the Gate: the notice opens the Run.
            seq: 0,
            task_id: run.task_id.map(|task| task.to_string()).unwrap_or_default(),
            request: FactoryGateWaitingMetadata::excerpt(&run.request),
            question: FactoryGateWaitingMetadata::excerpt(&stop_text(budget, stop)),
            asker: None,
            notify_only: false,
        },
        push: true,
    })
}
