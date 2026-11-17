//! Standing Rules at a Gate: whether one answers it on its own, with a Rule
//! Answer, and with what. A Run keeps the rules it was created with
//! (`factory_run.standing_rules`); when one of its Gates opens, the worker
//! reads what the rule of the Gate's kind requires and answers for it, or
//! leaves the Gate to a person.
//!
//! What no rule ever does, whatever a definition says:
//! - answer a Gate of a Tampered Run: its Control Files are a person's to
//!   approve, and a Rule Answer approves none;
//! - merge while the base branch has a Main Hold;
//! - send Blockers to be fixed once `MAX_RULE_FIX_ROUNDS` fix rounds in a
//!   row found Blockers again;
//! - answer a Gate of a Run created before Runs kept rules.

#[cfg(test)]
mod test;

use factory_records::domain::models::Outcome;
use factory_records::domain::standing_rules::{GateKind, OrchestratorDefinition, StandingRule};
use serde::{Deserialize, Serialize};

use crate::domain::flow::Entry;

/// The most fix rounds in a row the `fix_blockers` rule sends; Blockers a
/// review finds after them go to a person.
pub const MAX_RULE_FIX_ROUNDS: u16 = 2;

/// The kind of Gate `entry` asks, by the work it does: a review panel's
/// Gate is about its Blockers, `plan`'s about the plan, and
/// `ready_to_merge`'s about the merge. No rule decides any other Gate.
pub fn gate_kind(entry: &Entry) -> Option<GateKind> {
    if !entry.reviewers.is_empty() {
        return Some(GateKind::Blockers);
    }
    match entry.step.as_str() {
        "plan" => Some(GateKind::Plan),
        "ready_to_merge" => Some(GateKind::Merge),
        _ => None,
    }
}

/// What a Gate's rule reads of its Run when the Gate opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateFacts {
    /// The Gate's kind.
    pub kind: GateKind,
    /// The fix rounds in a row its review panel's Blockers went to since
    /// the panel's visits last started; nought for other Gates.
    pub fix_rounds: u16,
    /// Whether the Run is Tampered, as its Control Files were last read.
    pub tampered: bool,
    /// Whether the base branch the Run merges to has a Main Hold.
    pub held: bool,
}

/// A Standing Rule's answer to a Gate: the rule, and the answer it gives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleAnswer {
    /// The rule that answered.
    pub rule: StandingRule,
    /// Its answer, one the Gate offers: the outcome the Flow routes.
    pub answer: Outcome,
}

/// The answer the Run's `rules` give a Gate that offers `offered`, given
/// `facts`; none leaves it to a person. No rules (a Run created before
/// Runs kept them) answer nothing.
pub fn rule_answer(
    rules: Option<OrchestratorDefinition>,
    facts: &GateFacts,
    offered: &[Outcome],
) -> Option<RuleAnswer> {
    let rules = rules?;
    if !rules.decides(facts.kind) || facts.tampered {
        return None;
    }
    let answer = match facts.kind {
        GateKind::Plan => "approve",
        GateKind::Blockers if facts.fix_rounds >= MAX_RULE_FIX_ROUNDS => return None,
        GateKind::Blockers => "fix",
        // The merge Gate opens only once `ready_to_merge` found the pull
        // request's head still the commit `ci` called green, with its
        // checks still passing.
        GateKind::Merge if facts.held => return None,
        GateKind::Merge => "approve",
    };
    let answer = Outcome::try_from(answer.to_string()).ok()?;
    offered.contains(&answer).then(|| RuleAnswer {
        rule: facts.kind.rule(),
        answer,
    })
}
