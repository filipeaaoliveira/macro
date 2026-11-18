use super::*;

#[test]
fn a_definition_is_stored_as_the_kinds_its_rules_decide() {
    let definition = OrchestratorDefinition {
        plan: true,
        blockers: false,
        merge: true,
        owner_gates: false,
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
fn the_owner_section_is_stored_beside_the_kinds() {
    let definition = OrchestratorDefinition {
        plan: false,
        blockers: true,
        merge: false,
        owner_gates: true,
    };
    assert_eq!(definition.kinds(), ["blockers", "owner_gates"]);
    assert_eq!(
        OrchestratorDefinition::from_kinds(&definition.kinds()),
        Some(definition)
    );
}

#[test]
fn the_owner_section_is_off_by_default_and_when_a_definition_leaves_it_out() {
    assert!(!OrchestratorDefinition::DEFAULT.owner_gates);
    let sent: OrchestratorDefinition =
        serde_json::from_str(r#"{"plan": true, "blockers": true, "merge": false}"#)
            .expect("a definition without the section reads");
    assert!(!sent.owner_gates);
    let on: OrchestratorDefinition = serde_json::from_str(
        r#"{"plan": false, "blockers": true, "merge": false, "ownerGates": true}"#,
    )
    .expect("a definition with the section reads");
    assert!(on.owner_gates);
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
    assert!(default.decides(RuleGateKind::Blockers));
    assert!(!default.decides(RuleGateKind::Plan));
    assert!(!default.decides(RuleGateKind::Merge));
}

#[test]
fn each_kind_of_gate_has_its_own_rule() {
    assert_eq!(RuleGateKind::Plan.rule(), StandingRule::ApprovePlan);
    assert_eq!(RuleGateKind::Blockers.rule(), StandingRule::FixBlockers);
    assert_eq!(RuleGateKind::Merge.rule(), StandingRule::MergeWhenGreen);
    assert_eq!(StandingRule::MergeWhenGreen.to_string(), "merge_when_green");
    assert_eq!(
        "fix_blockers".parse::<StandingRule>().ok(),
        Some(StandingRule::FixBlockers)
    );
}
