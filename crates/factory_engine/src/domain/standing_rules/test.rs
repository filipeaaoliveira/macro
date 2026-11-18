use factory_records::domain::models::Outcome;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RuleGateKind, StandingRule};
use serde_json::json;

use super::{GateFacts, MAX_RULE_FIX_ROUNDS, RuleAnswer, rule_answer, rule_fix_limit};
use crate::domain::flow::{Flow, FlowError, StepUse};

mod repeated;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

const EVERY_RULE: OrchestratorDefinition = OrchestratorDefinition {
    stand_in: factory_records::domain::standing_rules::StandInKinds {
        plan: false,
        blockers: false,
    },
    plan: true,
    blockers: true,
    merge: true,
    owner_gates: false,
    task_thread: false,
    queue_limit: None,
    orchestrator: factory_records::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
};

fn facts(kind: RuleGateKind) -> GateFacts {
    GateFacts {
        kind,
        fix_rounds: 0,
        fix_limit: MAX_RULE_FIX_ROUNDS,
        tampered: false,
        held: false,
        repeated_blockers: false,
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
            &facts(RuleGateKind::Plan),
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
            &facts(RuleGateKind::Merge),
            &approve_or_revise()
        ),
        Some(RuleAnswer {
            rule: StandingRule::MergeWhenGreen,
            answer: outcome("approve"),
        })
    );
    assert_eq!(
        rule_answer(
            Some(EVERY_RULE),
            &facts(RuleGateKind::Blockers),
            &fix_or_stop()
        ),
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
        (RuleGateKind::Plan, approve_or_revise()),
        (RuleGateKind::Merge, approve_or_revise()),
        (RuleGateKind::Blockers, fix_or_stop()),
    ] {
        assert_eq!(rule_answer(Some(none), &facts(kind), &offered), None);
    }
    let merge_only = OrchestratorDefinition {
        stand_in: factory_records::domain::standing_rules::StandInKinds {
            plan: false,
            blockers: false,
        },
        merge: true,
        ..none
    };
    assert_eq!(
        rule_answer(
            Some(merge_only),
            &facts(RuleGateKind::Plan),
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
        rule_answer(None, &facts(RuleGateKind::Plan), &approve_or_revise()),
        None
    );
}

/// A Tampered Run's Control Files are a person's to approve: no rule
/// approves its plan or its merge.
#[test]
fn no_rule_approves_a_tampered_run_s_plan_or_merge() {
    for kind in [RuleGateKind::Plan, RuleGateKind::Merge] {
        let tampered = GateFacts {
            tampered: true,
            ..facts(kind)
        };
        assert_eq!(
            rule_answer(Some(EVERY_RULE), &tampered, &approve_or_revise()),
            None
        );
    }
}

/// A fix approves no Control Files, and `merge` reads them again before
/// it merges: a Tampered Run's Blockers still go to a fix.
#[test]
fn a_tampered_run_s_blockers_still_go_to_a_fix() {
    let tampered = GateFacts {
        tampered: true,
        ..facts(RuleGateKind::Blockers)
    };
    assert_eq!(
        rule_answer(Some(EVERY_RULE), &tampered, &fix_or_stop()),
        Some(RuleAnswer {
            rule: StandingRule::FixBlockers,
            answer: outcome("fix"),
        })
    );
}

#[test]
fn no_rule_merges_through_a_main_hold() {
    let held = GateFacts {
        held: true,
        ..facts(RuleGateKind::Merge)
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
        ..facts(RuleGateKind::Blockers)
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

/// A panel's rounds bound the rule's fixes: never more than the rounds
/// before its last, which asks nothing.
#[test]
fn a_panel_s_rounds_bound_the_rule_s_fixes() {
    let rounds = |n: u8| std::num::NonZeroU8::new(n);
    assert_eq!(rule_fix_limit(rounds(1)), 0);
    assert_eq!(rule_fix_limit(rounds(2)), 1);
    assert_eq!(rule_fix_limit(rounds(3)), 2);
    assert_eq!(rule_fix_limit(rounds(10)), 2);
    assert_eq!(rule_fix_limit(None), 2);
    let one_fix = GateFacts {
        fix_rounds: 1,
        fix_limit: rule_fix_limit(rounds(2)),
        ..facts(RuleGateKind::Blockers)
    };
    assert_eq!(
        rule_answer(Some(EVERY_RULE), &one_fix, &fix_or_stop()),
        None
    );
}

/// A rule answers only a Gate that offers exactly its kind's answers.
#[test]
fn a_rule_answers_only_a_gate_offering_its_kind_s_answers() {
    for offered in [
        vec![outcome("approve")],
        vec![outcome("approve"), outcome("revise"), outcome("stop")],
        vec![outcome("approve"), outcome("skip")],
        fix_or_stop(),
    ] {
        assert_eq!(
            rule_answer(Some(EVERY_RULE), &facts(RuleGateKind::Plan), &offered),
            None
        );
    }
    let reordered = [outcome("revise"), outcome("approve")];
    assert!(rule_answer(Some(EVERY_RULE), &facts(RuleGateKind::Plan), &reordered).is_some());
}

/// A Gate's kind is what its Flow names, never the work of its entry: a
/// plan entry's Gate with no kind has none, and a kind must offer its own
/// answers.
#[test]
fn a_gate_s_kind_is_what_its_flow_names() {
    let parse = |gate: serde_json::Value| {
        Flow::parse(
            json!({ "entries": [{ "id": "plan", "step": "plan", "role": "planner", "gate": gate }] }),
            |_| Some(StepUse::Work),
        )
    };
    let unnamed = parse(json!({ "question": "Approve?", "answers": ["approve", "revise"] }))
        .expect("valid flow");
    assert_eq!(
        unnamed.entries[0].gate.as_ref().and_then(|gate| gate.kind),
        None
    );
    let named =
        parse(json!({ "kind": "plan", "question": "Approve?", "answers": ["approve", "revise"] }))
            .expect("valid flow");
    assert_eq!(
        named.entries[0].gate.as_ref().and_then(|gate| gate.kind),
        Some(RuleGateKind::Plan)
    );
    let wrong_answers =
        parse(json!({ "kind": "plan", "question": "Approve?", "answers": ["approve", "skip"] }));
    assert!(matches!(
        wrong_answers,
        Err(FlowError::GateKindAnswers { .. })
    ));
    let unknown = parse(json!({ "kind": "question", "question": "Which?", "answers": ["a", "b"] }));
    assert!(matches!(unknown, Err(FlowError::Unreadable(_))));
}

/// The built-in Flows name each Gate a rule may answer.
#[test]
fn the_built_in_flows_name_their_gates_kinds() {
    let kinds = |name: &str| -> Vec<(String, Option<RuleGateKind>)> {
        crate::domain::built_in_flows::BuiltInFlow::named(name)
            .expect("built in")
            .read(|_| Some(StepUse::Work))
            .expect("valid")
            .entries
            .into_iter()
            .filter_map(|entry| {
                entry
                    .gate
                    .map(|gate| (entry.id.as_str().to_string(), gate.kind))
            })
            .collect()
    };
    assert_eq!(
        kinds("code"),
        [
            ("plan".to_string(), Some(RuleGateKind::Plan)),
            ("review".to_string(), Some(RuleGateKind::Blockers)),
            ("merge-gate".to_string(), Some(RuleGateKind::Merge)),
        ]
    );
    assert_eq!(
        kinds("plan"),
        [("plan".to_string(), Some(RuleGateKind::Plan))]
    );
}
