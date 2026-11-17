//! Standing Rules at a Gate: whether one answers it on its own, with a Rule
//! Answer, and with what. A Run keeps the rules it was created with
//! (`factory_run.standing_rules`); when one of its Gates opens, the worker
//! reads what the rule of the Gate's kind requires and answers for it, or
//! leaves the Gate to a person.
//!
//! A Gate's kind is set by its Flow (`GateDefinition::kind`), never read
//! from the work of its entry: a Gate with no kind, or one offering other
//! answers than its kind's, is never answered by a rule.
//!
//! What no rule ever does, whatever a definition says:
//! - approve a Tampered Run's plan or merge: its Control Files are a
//!   person's to approve, and a Rule Answer approves none. Its Blockers
//!   may still go to a fix, which approves nothing, and `merge` reads the
//!   Control Files again before it merges;
//! - merge while the base branch has a Main Hold;
//! - send Blockers to more fixes in a row than [`rule_fix_limit`] allows;
//! - answer a Gate of a Run created before Runs kept rules.

#[cfg(test)]
mod test;

use factory_records::domain::models::Outcome;
use factory_records::domain::standing_rules::{GateKind, OrchestratorDefinition, StandingRule};
use serde::{Deserialize, Serialize};

/// The most fix rounds in a row the `fix_blockers` rule sends; Blockers a
/// review finds after them go to a person.
pub const MAX_RULE_FIX_ROUNDS: u16 = 2;

/// The fix rounds in a row the `fix_blockers` rule may send for a review
/// panel that runs at most `max_visits` rounds: [`MAX_RULE_FIX_ROUNDS`],
/// and never more than the panel's rounds leave room for. A panel's last
/// round asks nothing (its Blockers route `unresolved`), so with the
/// `code` Flow's three rounds the rule fixes twice and the third round's
/// Blockers end the Run for a person; a panel bound to more rounds reaches
/// the cap, and its next Blockers wait at the Gate.
pub fn rule_fix_limit(max_visits: Option<std::num::NonZeroU8>) -> u16 {
    match max_visits {
        Some(rounds) => MAX_RULE_FIX_ROUNDS.min(u16::from(rounds.get()).saturating_sub(1)),
        None => MAX_RULE_FIX_ROUNDS,
    }
}

/// The answers a Gate of `kind` must offer, exactly, for its rule to
/// answer it: the first is the rule's answer.
pub fn kind_answers(kind: GateKind) -> [&'static str; 2] {
    match kind {
        GateKind::Plan | GateKind::Merge => ["approve", "revise"],
        GateKind::Blockers => ["fix", "stop"],
    }
}

/// Whether `offered` is exactly the answers a Gate of `kind` offers, in
/// any order.
pub fn offers_kind_answers(kind: GateKind, offered: &[Outcome]) -> bool {
    let expected = kind_answers(kind);
    offered.len() == expected.len()
        && expected
            .iter()
            .all(|answer| offered.iter().any(|offer| offer.as_str() == *answer))
}

/// What a Gate's rule reads of its Run when the Gate opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateFacts {
    /// The Gate's kind.
    pub kind: GateKind,
    /// The fix rounds in a row its review panel's Blockers went to since
    /// the panel's visits last started; nought for other Gates.
    pub fix_rounds: u16,
    /// The fix rounds in a row the rule may send ([`rule_fix_limit`]).
    pub fix_limit: u16,
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
    if !rules.decides(facts.kind) || !offers_kind_answers(facts.kind, offered) {
        return None;
    }
    let answer = match facts.kind {
        GateKind::Plan | GateKind::Merge if facts.tampered => return None,
        GateKind::Plan => "approve",
        GateKind::Blockers if facts.fix_rounds >= facts.fix_limit => return None,
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
