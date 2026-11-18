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
//! - with the owner section on, send a round's Blockers to a fix when they
//!   are the round before's, unchanged after the rule sent those to a fix
//!   ([`repeated_blockers`]): another fix would repeat the last, so the
//!   Gate waits for the Run's owner;
//! - answer a Gate of a Run created before Runs kept rules.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, Step};
use factory_records::domain::review_rounds::ReviewRound;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RuleGateKind, StandingRule};
use serde::{Deserialize, Serialize};

use crate::domain::review_rounds::same_blockers;

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
pub fn kind_answers(kind: RuleGateKind) -> [&'static str; 2] {
    match kind {
        RuleGateKind::Plan | RuleGateKind::Merge => ["approve", "revise"],
        RuleGateKind::Blockers => ["fix", "stop"],
    }
}

/// Whether `offered` is exactly the answers a Gate of `kind` offers, in
/// any order.
pub fn offers_kind_answers(kind: RuleGateKind, offered: &[Outcome]) -> bool {
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
    pub kind: RuleGateKind,
    /// The fix rounds in a row its review panel's Blockers went to since
    /// the panel's visits last started; nought for other Gates.
    pub fix_rounds: u16,
    /// The fix rounds in a row the rule may send ([`rule_fix_limit`]).
    pub fix_limit: u16,
    /// Whether the Run is Tampered, as its Control Files were last read.
    pub tampered: bool,
    /// Whether the base branch the Run merges to has a Main Hold.
    pub held: bool,
    /// For a review panel's Gate, whether its round's Blockers are the
    /// round before's, unchanged after the rule sent those to a fix
    /// ([`repeated_blockers`]); false for other Gates.
    pub repeated_blockers: bool,
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
        RuleGateKind::Plan | RuleGateKind::Merge if facts.tampered => return None,
        RuleGateKind::Plan => "approve",
        RuleGateKind::Blockers if facts.fix_rounds >= facts.fix_limit => return None,
        RuleGateKind::Blockers if rules.owner_gates && facts.repeated_blockers => return None,
        RuleGateKind::Blockers => "fix",
        // The merge Gate opens only once `ready_to_merge` found the pull
        // request's head still the commit `ci` called green, with its
        // checks still passing.
        RuleGateKind::Merge if facts.held => return None,
        RuleGateKind::Merge => "approve",
    };
    let answer = Outcome::try_from(answer.to_string()).ok()?;
    offered.contains(&answer).then(|| RuleAnswer {
        rule: facts.kind.rule(),
        answer,
    })
}

/// Whether the review round the Gate at `seq` asks about found the round
/// before's Blockers again, unchanged, after the `fix_blockers` rule sent
/// those to a fix: a second rule fix would most likely repeat the first.
/// `rounds` and `steps` are the Run's, in order.
///
/// The round asked about is the latest that started at or before `seq`
/// (its Gate is on its latest Step), and the round before is the latest
/// earlier round of the same review panel (the entry of its first Step)
/// one visit before it, so a round after a reset, and a round of another
/// panel, are never compared with it. That
/// round's Blockers went to a fix by the rule when a Step between the two
/// rounds has a Gate whose Rule Answer is `fix_blockers`: a person's fix
/// is theirs to repeat. Which Blockers count as the same is
/// [`same_blockers`]'s to say.
pub fn repeated_blockers(rounds: &[ReviewRound], steps: &[Step], seq: u16) -> bool {
    let Some(at) = rounds.iter().rposition(|round| round.seq <= seq) else {
        return false;
    };
    let round = &rounds[at];
    let entry_of = |seq: u16| {
        steps
            .iter()
            .find(|step| step.seq == seq)
            .map(|step| &step.entry)
    };
    let Some(entry) = entry_of(round.seq) else {
        return false;
    };
    let Some(before) = rounds[..at]
        .iter()
        .rev()
        .find(|before| entry_of(before.seq) == Some(entry) && before.visit + 1 == round.visit)
    else {
        return false;
    };

    let fixed_by_rule = steps.iter().any(|step| {
        let between = step.seq >= before.seq && step.seq < round.seq;
        let rule = step
            .gate
            .as_ref()
            .and_then(|gate| gate.answer.as_ref())
            .and_then(|answer| answer.rule);
        between && rule == Some(StandingRule::FixBlockers)
    });
    fixed_by_rule && same_blockers(before, round)
}
