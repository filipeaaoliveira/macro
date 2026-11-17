use super::*;

#[test]
fn a_definition_is_stored_as_the_kinds_its_rules_decide() {
    let definition = OrchestratorDefinition {
        plan: true,
        blockers: false,
        merge: true,
    };
    assert_eq!(definition.kinds(), ["plan", "merge"]);
    assert_eq!(
        OrchestratorDefinition::from_kinds(&definition.kinds()),
        Some(definition)
    );
    assert_eq!(
        OrchestratorDefinition::from_kinds(&[]),
        Some(OrchestratorDefinition::default())
    );
}

#[test]
fn a_stored_kind_this_code_does_not_know_does_not_read() {
    assert_eq!(
        OrchestratorDefinition::from_kinds(&["plan".to_string(), "deploy".to_string()]),
        None
    );
}

#[test]
fn the_default_fixes_blockers_and_leaves_the_plan_and_the_merge_to_a_person() {
    let default = OrchestratorDefinition::DEFAULT;
    assert!(default.decides(GateKind::Blockers));
    assert!(!default.decides(GateKind::Plan));
    assert!(!default.decides(GateKind::Merge));
}

#[test]
fn each_kind_of_gate_has_its_own_rule() {
    assert_eq!(GateKind::Plan.rule(), StandingRule::ApprovePlan);
    assert_eq!(GateKind::Blockers.rule(), StandingRule::FixBlockers);
    assert_eq!(GateKind::Merge.rule(), StandingRule::MergeWhenGreen);
    assert_eq!(StandingRule::MergeWhenGreen.to_string(), "merge_when_green");
    assert_eq!(
        "fix_blockers".parse::<StandingRule>().ok(),
        Some(StandingRule::FixBlockers)
    );
}
