use serde_json::json;

use super::{Flow, FlowError, StepName, StepUse};

/// A worker that replies itself, and has an agent write.
fn parse(definition: serde_json::Value) -> Result<Flow, FlowError> {
    Flow::parse(definition, |step: &StepName| match step.as_str() {
        "reply" => Some(StepUse::Work),
        "write" => Some(StepUse::Agent),
        _ => None,
    })
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
fn an_entry_an_agent_does_must_name_its_role() {
    let refused = parse(json!({ "entries": [{ "id": "write", "step": "write" }] }));
    let named = parse(json!({
        "entries": [{ "id": "write", "step": "write", "role": "writer" }]
    }));

    assert!(matches!(refused, Err(FlowError::NoRole(_))));
    assert!(named.is_ok());
}

#[test]
fn an_unknown_key_is_refused() {
    let refused = parse(json!({
        "entries": [{ "id": "reply", "step": "reply", "retries": 3 }]
    }));

    assert!(matches!(refused, Err(FlowError::Unreadable(_))));
}

#[test]
fn a_gate_needs_a_question_and_an_answer() {
    let refused = parse(json!({
        "entries": [{
            "id": "reply",
            "step": "reply",
            "gate": { "question": " ", "answers": ["approve"] }
        }]
    }));

    assert!(matches!(refused, Err(FlowError::IncompleteGate(_))));
}

#[test]
fn a_gate_offers_each_answer_once() {
    let refused = parse(json!({
        "entries": [{
            "id": "reply",
            "step": "reply",
            "gate": { "question": "Send it?", "answers": ["approve", "approve"] }
        }]
    }));

    assert!(matches!(refused, Err(FlowError::DuplicateAnswer { .. })));
}
