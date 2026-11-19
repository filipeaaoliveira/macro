use super::*;

#[test]
fn a_definition_is_stored_as_the_kinds_its_rules_decide() {
    let definition = OrchestratorDefinition {
        plan: true,
        blockers: false,
        merge: true,
        owner_gates: false,
        quiet_gates: false,
        task_thread: false,
        queue_limit: None,
        orchestrator: crate::domain::standing_rules::OrchestratorKinds {
            plan: false,
            blockers: false,
        },
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
fn the_owner_section_is_stored_beside_the_kinds() {
    let definition = OrchestratorDefinition {
        plan: false,
        blockers: true,
        merge: false,
        owner_gates: true,
        quiet_gates: false,
        task_thread: false,
        queue_limit: None,
        orchestrator: crate::domain::standing_rules::OrchestratorKinds {
            plan: false,
            blockers: false,
        },
        stand_in: StandInKinds::default(),
    };
    assert_eq!(definition.kinds(), ["blockers", "owner_gates"]);
    assert_eq!(
        OrchestratorDefinition::from_kinds(&definition.kinds()),
        Some(definition)
    );
}

#[test]
fn the_owner_section_is_off_by_default_and_when_a_definition_leaves_it_out() {
    assert!(
        !OrchestratorDefinition::DEFAULT
            .kinds()
            .contains(&"owner_gates".to_string())
    );
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
fn a_definition_sent_without_a_queue_limit_sets_none() {
    let sent: OrchestratorDefinition =
        serde_json::from_str(r#"{"plan": true, "blockers": true, "merge": false}"#)
            .expect("a definition without the limit reads");
    assert_eq!(sent.queue_limit, None);
    let limited: OrchestratorDefinition = serde_json::from_str(
        r#"{"plan": false, "blockers": true, "merge": false, "queueLimit": 4}"#,
    )
    .expect("a definition with the limit reads");
    assert_eq!(limited.queue_limit, Some(4));
    // The limit is not one of the stored names.
    assert_eq!(limited.kinds(), ["blockers"]);
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
        owner_gates: false,
        quiet_gates: false,
        task_thread: false,
        queue_limit: None,
        orchestrator: crate::domain::standing_rules::OrchestratorKinds {
            plan: false,
            blockers: false,
        },
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
        owner_gates: false,
        quiet_gates: false,
        task_thread: false,
        queue_limit: None,
        orchestrator: crate::domain::standing_rules::OrchestratorKinds {
            plan: false,
            blockers: false,
        },
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

#[test]
fn the_owner_section_and_the_stand_in_are_stored_together() {
    let definition = OrchestratorDefinition {
        plan: true,
        blockers: false,
        merge: false,
        owner_gates: true,
        quiet_gates: false,
        task_thread: false,
        queue_limit: None,
        orchestrator: crate::domain::standing_rules::OrchestratorKinds {
            plan: false,
            blockers: false,
        },
        stand_in: StandInKinds {
            plan: false,
            blockers: true,
        },
    };
    assert_eq!(
        definition.kinds(),
        ["plan", "owner_gates", "stand_in_blockers"]
    );
    assert_eq!(
        OrchestratorDefinition::from_kinds(&definition.kinds()),
        Some(definition)
    );
}

#[test]
fn the_task_thread_section_is_stored_beside_the_kinds_and_off_by_default() {
    let definition = OrchestratorDefinition {
        task_thread: true,
        ..OrchestratorDefinition::DEFAULT
    };
    assert_eq!(definition.kinds(), ["blockers", "task_thread"]);
    assert_eq!(
        OrchestratorDefinition::from_kinds(&definition.kinds()),
        Some(definition)
    );
    assert_eq!(OrchestratorDefinition::DEFAULT.kinds(), ["blockers"]);
    let sent: OrchestratorDefinition = serde_json::from_str(
        r#"{"plan": false, "blockers": true, "merge": false, "ownerGates": true}"#,
    )
    .expect("a definition without the section reads");
    assert!(!sent.task_thread);
    let on: OrchestratorDefinition = serde_json::from_str(
        r#"{"plan": false, "blockers": true, "merge": false, "taskThread": true}"#,
    )
    .expect("a definition with the section reads");
    assert!(on.task_thread);
}

/// Gates notify the owner unless a definition says it is quiet: an
/// opt-out, stored only when on, so a definition kept or sent without it,
/// and the factory's default, notify.
#[test]
fn quiet_gates_are_an_opt_out_stored_beside_the_kinds() {
    const { assert!(!OrchestratorDefinition::DEFAULT.quiet_gates) };
    assert!(
        !OrchestratorDefinition::from_kinds(&["blockers".to_string()])
            .expect("known")
            .quiet_gates
    );
    let quiet = OrchestratorDefinition {
        quiet_gates: true,
        ..OrchestratorDefinition::DEFAULT
    };
    assert_eq!(quiet.kinds(), ["blockers", "quiet_gates"]);
    assert_eq!(
        OrchestratorDefinition::from_kinds(&quiet.kinds()),
        Some(quiet)
    );
    let sent: OrchestratorDefinition =
        serde_json::from_str(r#"{"plan": false, "blockers": true, "merge": false}"#)
            .expect("a definition without it reads");
    assert!(!sent.quiet_gates);
    let off: OrchestratorDefinition = serde_json::from_str(
        r#"{"plan": false, "blockers": true, "merge": false, "quietGates": true}"#,
    )
    .expect("a definition with it reads");
    assert!(off.quiet_gates);
    assert_eq!(
        serde_json::to_value(off).expect("serializes")["quietGates"],
        true
    );
}

#[test]
fn the_kinds_the_orchestrator_may_approve_are_stored_beside_the_rules() {
    let definition = OrchestratorDefinition {
        orchestrator: OrchestratorKinds {
            plan: true,
            blockers: true,
        },
        ..OrchestratorDefinition::DEFAULT
    };
    assert_eq!(
        definition.kinds(),
        ["blockers", "orchestrator_plan", "orchestrator_blockers"]
    );
    assert_eq!(
        OrchestratorDefinition::from_kinds(&definition.kinds()),
        Some(definition)
    );
    assert!(definition.orchestrator_approves(RuleGateKind::Plan));
    assert!(!definition.orchestrator_approves(RuleGateKind::Merge));
    assert!(!OrchestratorDefinition::DEFAULT.orchestrator_approves(RuleGateKind::Plan));
}
