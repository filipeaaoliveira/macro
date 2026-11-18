use super::*;

#[test]
fn a_definition_is_stored_as_the_kinds_its_rules_decide() {
    let definition = OrchestratorDefinition {
        plan: true,
        blockers: false,
        merge: true,
        stand_in: StandInKinds::default(),
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

#[test]
fn the_kinds_the_stand_in_answers_are_stored_beside_the_rules() {
    let definition = OrchestratorDefinition {
        plan: false,
        blockers: true,
        merge: false,
        stand_in: StandInKinds {
            plan: true,
            blockers: false,
        },
    };
    assert_eq!(definition.kinds(), ["blockers", "stand_in_plan"]);
    assert_eq!(
        OrchestratorDefinition::from_kinds(&definition.kinds()),
        Some(definition)
    );
    assert!(definition.stands_in(RuleGateKind::Plan));
    assert!(!definition.stands_in(RuleGateKind::Blockers));
}

#[test]
fn the_stand_in_never_answers_a_merge_and_is_off_by_default() {
    let every_kind = OrchestratorDefinition {
        plan: true,
        blockers: true,
        merge: true,
        stand_in: StandInKinds {
            plan: true,
            blockers: true,
        },
    };
    assert!(!every_kind.stands_in(RuleGateKind::Merge));
    let default = OrchestratorDefinition::DEFAULT;
    assert!(!default.stands_in(RuleGateKind::Plan));
    assert!(!default.stands_in(RuleGateKind::Blockers));
}

#[test]
fn a_definition_sent_without_the_stand_in_leaves_it_off() {
    let definition: OrchestratorDefinition =
        serde_json::from_str(r#"{"plan": true, "blockers": true, "merge": false}"#)
            .expect("an older definition reads");
    assert_eq!(definition.stand_in, StandInKinds::default());
}
