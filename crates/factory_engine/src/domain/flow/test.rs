use serde_json::json;

use factory_records::domain::models::Outcome;

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

#[test]
fn a_review_panel_fills_its_agent_step_with_its_reviewers() {
    let flow = parse(json!({
        "entries": [{ "id": "write", "step": "write", "reviewers": ["correctness", "tests"] }]
    }))
    .expect("a panel names its roles");

    let reviewers: Vec<_> = flow.entries[0]
        .reviewers
        .iter()
        .map(|role| role.as_str())
        .collect();
    assert_eq!(reviewers, ["correctness", "tests"]);
}

#[test]
fn a_review_panel_names_no_single_role_no_gate_on_done_and_no_reviewer_twice() {
    let with_role = parse(json!({
        "entries": [{ "id": "write", "step": "write", "role": "writer", "reviewers": ["tests"] }]
    }));
    let with_gate = parse(json!({
        "entries": [{
            "id": "write",
            "step": "write",
            "reviewers": ["tests"],
            "gate": { "question": "Merge?", "answers": ["approve"] }
        }]
    }));
    let twice = parse(json!({
        "entries": [{ "id": "write", "step": "write", "reviewers": ["tests", "tests"] }]
    }));

    assert!(matches!(with_role, Err(FlowError::ReviewersAndRole(_))));
    assert!(matches!(with_gate, Err(FlowError::ReviewersAndRole(_))));
    assert!(matches!(twice, Err(FlowError::DuplicateReviewer { .. })));
}

/// A review panel's Gate asks about one of its rounds' outcomes, which it
/// names; a Gate written as before opens on `done`.
#[test]
fn a_review_panel_s_gate_opens_on_the_outcome_it_names() {
    let flow = parse(json!({
        "entries": [{
            "id": "review",
            "step": "write",
            "reviewers": ["tests"],
            "gate": { "question": "Fix?", "answers": ["fix", "stop"], "after": "changes" }
        }, {
            "id": "plan",
            "step": "write",
            "role": "planner",
            "gate": { "question": "Approve?", "answers": ["approve"] }
        }]
    }))
    .expect("a panel's gate names its outcome");

    let opened_by = |index: usize| {
        flow.entries[index]
            .gate
            .as_ref()
            .map(|gate| gate.opened_by())
    };
    assert_eq!(
        opened_by(0),
        Some(Outcome::try_from("changes".to_string()).expect("valid"))
    );
    assert_eq!(opened_by(1), Some(Outcome::done()));
    // A Gate with no `after` is written as before, so a Run carrying it
    // reads the same.
    let written = serde_json::to_value(&flow.entries[1]).expect("writes");
    assert!(written["gate"].get("after").is_none());
}

/// A review panel's Gate opens after an outcome a round ends with; one
/// after `done` would never open.
#[test]
fn a_review_panel_s_gate_opens_after_a_round_s_outcome() {
    let refused = parse(json!({
        "entries": [{
            "id": "review",
            "step": "write",
            "reviewers": ["tests"],
            "gate": { "question": "Fix?", "answers": ["fix", "stop"], "after": "done" }
        }]
    }));

    assert!(matches!(
        refused,
        Err(FlowError::GateAfterNoRoundOutcome(_))
    ));
}

/// No Flow's Gate opens after an agent's question, on a Step or a review
/// panel: the question has a Gate of its own, which only the Run's owner
/// answers.
#[test]
fn a_gate_after_a_question_is_refused() {
    let on_a_step = parse(json!({
        "entries": [{
            "id": "write",
            "step": "write",
            "role": "writer",
            "gate": { "question": "Go on?", "answers": ["approve", "revise"], "after": "question" }
        }]
    }));
    let on_a_panel = parse(json!({
        "entries": [{
            "id": "review",
            "step": "write",
            "reviewers": ["tests"],
            "gate": { "question": "Fix?", "answers": ["fix", "stop"], "after": "question" }
        }]
    }));

    assert!(matches!(on_a_step, Err(FlowError::GateAfterQuestion(_))));
    // A round never ends `question`, so a panel's is refused as any Gate
    // after an outcome no round ends with.
    assert!(matches!(
        on_a_panel,
        Err(FlowError::GateAfterNoRoundOutcome(_))
    ));
}

/// A deployment's bound on review rounds replaces the `max_visits` of each
/// review panel, and changes nothing else.
#[test]
fn a_bound_on_review_rounds_changes_only_the_review_panels() {
    let flow = parse(json!({
        "entries": [
            { "id": "write", "step": "write", "role": "writer", "max_visits": 2 },
            { "id": "review", "step": "write", "reviewers": ["tests"], "max_visits": 3,
              "on": { "unresolved": [{ "to": { "kind": "end", "outcome": "manual" } }] } }
        ]
    }))
    .expect("reads");
    let mut bounded = flow.clone();

    bounded.bound_review_rounds(std::num::NonZeroU8::new(5).expect("not zero"));

    assert_eq!(bounded.entries[0], flow.entries[0]);
    assert_eq!(bounded.entries[1].max_visits, std::num::NonZeroU8::new(5));
}

/// A review panel bounded in rounds says where its last round's unresolved
/// Blockers go.
#[test]
fn a_bounded_review_panel_routes_unresolved() {
    let refused = parse(json!({
        "entries": [{ "id": "write", "step": "write", "reviewers": ["tests"], "max_visits": 2 }]
    }));

    assert!(matches!(refused, Err(FlowError::NoUnresolvedRoute(_))));
}

#[test]
fn a_notify_only_gate_offers_no_answer_and_keeps_its_question() {
    let gate = |gate: serde_json::Value| {
        parse(json!({
            "entries": [{ "id": "reply", "step": "reply", "gate": gate }]
        }))
    };

    let told = gate(json!({ "question": "The reply is out.", "notify_only": true }))
        .expect("a notify-only gate with a question reads");
    let read = told.entries[0].gate.as_ref().expect("gate");
    assert!(read.notify_only);
    assert!(read.answers.is_empty());
    assert_eq!(
        serde_json::to_value(read).expect("written"),
        json!({ "question": "The reply is out.", "answers": [], "notify_only": true })
    );

    let answered = gate(json!({
        "question": "The reply is out.",
        "answers": ["approve"],
        "notify_only": true
    }));
    assert!(matches!(answered, Err(FlowError::NotifyOnlyGateAnswers(_))));
    let kept =
        gate(json!({ "question": "The reply is out.", "notify_only": true, "person_only": true }));
    assert!(matches!(kept, Err(FlowError::NotifyOnlyGateAnswers(_))));
    let silent = gate(json!({ "question": " ", "notify_only": true }));
    assert!(matches!(silent, Err(FlowError::IncompleteGate(_))));
    let unanswerable = gate(json!({ "question": "Go on?" }));
    assert!(matches!(unanswerable, Err(FlowError::IncompleteGate(_))));
}

/// No Flow's Gate opens after `no_result`: an agent that stopped without a
/// result has a Gate of its own, which only the Run's owner answers.
#[test]
fn a_gate_after_no_result_is_refused() {
    let flow = parse(json!({
        "entries": [{
            "id": "write",
            "step": "write",
            "role": "writer",
            "gate": { "question": "Go on?", "answers": ["approve", "revise"], "after": "no_result" }
        }]
    }));

    assert!(matches!(flow, Err(FlowError::GateAfterNoResult(_))));
}
