use factory_records::domain::models::Outcome;
use factory_records::domain::standing_rules::{GateKind, OrchestratorDefinition, StandingRule};
use serde_json::json;

use super::{GateFacts, MAX_RULE_FIX_ROUNDS, RuleAnswer, gate_kind, rule_answer};
use crate::domain::flow::{Flow, StepUse};

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

const EVERY_RULE: OrchestratorDefinition = OrchestratorDefinition {
    plan: true,
    blockers: true,
    merge: true,
};

fn facts(kind: GateKind) -> GateFacts {
    GateFacts {
        kind,
        fix_rounds: 0,
        tampered: false,
        held: false,
    }
}

fn approve_or_revise() -> Vec<Outcome> {
    vec![outcome("approve"), outcome("revise")]
}

fn fix_or_stop() -> Vec<Outcome> {
    vec![outcome("fix"), outcome("stop")]
}

#[test]
fn each_rule_answers_its_own_kind_of_gate() {
    assert_eq!(
        rule_answer(
            Some(EVERY_RULE),
            &facts(GateKind::Plan),
            &approve_or_revise()
        ),
        Some(RuleAnswer {
            rule: StandingRule::ApprovePlan,
            answer: outcome("approve"),
        })
    );
    assert_eq!(
        rule_answer(
            Some(EVERY_RULE),
            &facts(GateKind::Merge),
            &approve_or_revise()
        ),
        Some(RuleAnswer {
            rule: StandingRule::MergeWhenGreen,
            answer: outcome("approve"),
        })
    );
    assert_eq!(
        rule_answer(Some(EVERY_RULE), &facts(GateKind::Blockers), &fix_or_stop()),
        Some(RuleAnswer {
            rule: StandingRule::FixBlockers,
            answer: outcome("fix"),
        })
    );
}

#[test]
fn a_kind_its_definition_leaves_off_waits_for_a_person() {
    let none = OrchestratorDefinition::default();
    for (kind, offered) in [
        (GateKind::Plan, approve_or_revise()),
        (GateKind::Merge, approve_or_revise()),
        (GateKind::Blockers, fix_or_stop()),
    ] {
        assert_eq!(rule_answer(Some(none), &facts(kind), &offered), None);
    }
    let merge_only = OrchestratorDefinition {
        merge: true,
        ..none
    };
    assert_eq!(
        rule_answer(
            Some(merge_only),
            &facts(GateKind::Plan),
            &approve_or_revise()
        ),
        None
    );
}

/// A Run created before Runs kept rules has none: a person answers every
/// Gate it opens, as before.
#[test]
fn a_run_with_no_rules_waits_for_a_person() {
    assert_eq!(
        rule_answer(None, &facts(GateKind::Plan), &approve_or_revise()),
        None
    );
}

/// A Tampered Run's Control Files are a person's to approve: no rule
/// answers any of its Gates.
#[test]
fn no_rule_answers_a_tampered_run() {
    for (kind, offered) in [
        (GateKind::Plan, approve_or_revise()),
        (GateKind::Merge, approve_or_revise()),
        (GateKind::Blockers, fix_or_stop()),
    ] {
        let tampered = GateFacts {
            tampered: true,
            ..facts(kind)
        };
        assert_eq!(rule_answer(Some(EVERY_RULE), &tampered, &offered), None);
    }
}

#[test]
fn no_rule_merges_through_a_main_hold() {
    let held = GateFacts {
        held: true,
        ..facts(GateKind::Merge)
    };
    assert_eq!(
        rule_answer(Some(EVERY_RULE), &held, &approve_or_revise()),
        None
    );
}

/// Blockers found again after two fix rounds go to a person, not to a
/// third fix.
#[test]
fn blockers_after_two_fix_rounds_go_to_a_person() {
    let after = |fix_rounds| GateFacts {
        fix_rounds,
        ..facts(GateKind::Blockers)
    };
    assert!(rule_answer(Some(EVERY_RULE), &after(1), &fix_or_stop()).is_some());
    assert_eq!(MAX_RULE_FIX_ROUNDS, 2);
    assert_eq!(
        rule_answer(Some(EVERY_RULE), &after(2), &fix_or_stop()),
        None
    );
    assert_eq!(
        rule_answer(Some(EVERY_RULE), &after(3), &fix_or_stop()),
        None
    );
}

#[test]
fn a_rule_gives_only_an_answer_the_gate_offers() {
    assert_eq!(
        rule_answer(
            Some(EVERY_RULE),
            &facts(GateKind::Plan),
            &[outcome("revise")]
        ),
        None
    );
}

#[test]
fn a_gate_s_kind_is_the_work_its_entry_does() {
    let flow = Flow::parse(
        json!({
            "entries": [
                { "id": "plan", "step": "plan", "role": "planner" },
                { "id": "review", "step": "review", "reviewers": ["tests"] },
                { "id": "merge-gate", "step": "ready_to_merge" },
                { "id": "write", "step": "write", "role": "writer" }
            ]
        }),
        |_| Some(StepUse::Work),
    )
    .expect("valid flow");
    let kinds: Vec<_> = flow.entries.iter().map(gate_kind).collect();
    assert_eq!(
        kinds,
        [
            Some(GateKind::Plan),
            Some(GateKind::Blockers),
            Some(GateKind::Merge),
            None
        ]
    );
}
