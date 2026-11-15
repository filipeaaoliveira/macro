use serde_json::json;

use super::{Flow, FlowError, StepName};

fn parse(definition: serde_json::Value) -> Result<Flow, FlowError> {
    Flow::parse(definition, |step: &StepName| step.as_str() == "reply")
}

#[test]
fn a_flow_with_no_entries_is_refused() {
    let refused = parse(json!({ "entries": [] }));

    assert_eq!(refused, Err(FlowError::NoEntries));
}

#[test]
fn two_entries_with_one_id_are_refused() {
    let refused = parse(json!({
        "entries": [
            { "id": "reply", "step": "reply" },
            { "id": "reply", "step": "reply" }
        ]
    }));

    assert!(matches!(refused, Err(FlowError::DuplicateEntry(_))));
}

#[test]
fn a_target_must_name_an_entry_of_the_flow() {
    let refused = parse(json!({
        "entries": [{
            "id": "reply",
            "step": "reply",
            "on": { "done": [{ "to": { "kind": "entry", "entry": "approve" } }] }
        }]
    }));

    assert!(matches!(refused, Err(FlowError::UnknownEntry { .. })));
}

#[test]
fn a_reset_must_name_an_entry_of_the_flow() {
    let refused = parse(json!({
        "entries": [{
            "id": "reply",
            "step": "reply",
            "on": { "done": [{ "to": { "kind": "end", "outcome": "success" }, "reset": ["review"] }] }
        }]
    }));

    assert!(matches!(refused, Err(FlowError::UnknownEntry { .. })));
}

#[test]
fn a_fact_must_be_declared_before_a_transition_sets_it() {
    let refused = parse(json!({
        "entries": [{
            "id": "reply",
            "step": "reply",
            "on": { "done": [{ "to": { "kind": "end", "outcome": "success" }, "set": { "held": true } }] }
        }]
    }));

    assert!(matches!(refused, Err(FlowError::UnknownFact { .. })));
}

#[test]
fn an_entry_must_run_a_step_the_worker_serves() {
    let refused = parse(json!({ "entries": [{ "id": "plan", "step": "plan" }] }));

    assert!(matches!(refused, Err(FlowError::UnknownStep { .. })));
}

#[test]
fn an_unknown_key_is_refused() {
    let refused = parse(json!({
        "entries": [{ "id": "reply", "step": "reply", "retries": 3 }]
    }));

    assert!(matches!(refused, Err(FlowError::Unreadable(_))));
}
