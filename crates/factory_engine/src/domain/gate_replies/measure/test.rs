//! The replay measure, over a scripted model.

use chrono::Utc;
use factory_records::domain::gate_replies::AnsweredGate;
use factory_records::domain::models::{EntryId, GateKind, Outcome, RunId};
use macro_user_id::user_id::MacroUserIdStr;

use super::{AtThreshold, ENOUGH_DECIDED, ENOUGH_GATES, Measure, describe, replay};
use crate::domain::fakes::FakeGateModel;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

/// A plan Gate Julia answered `answer`, writing `note`.
fn plan_gate(answer: &str, note: Option<&str>, kind: GateKind) -> AnsweredGate {
    AnsweredGate {
        run_id: RunId::generate(),
        seq: 2,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        owner: MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid"),
        flow: Some("code".to_string()),
        flow_definition: None,
        request: "Fix the login bug".to_string(),
        kind,
        question: "Approve this plan?".to_string(),
        answers: vec![outcome("approve"), outcome("revise")],
        output: "The plan".to_string(),
        plan: Some("Change the session check.".to_string()),
        answer: outcome(answer),
        note: note.map(str::to_string),
        answered_at: Utc::now(),
    }
}

fn row(measure: &Measure, threshold: u8) -> AtThreshold {
    measure
        .thresholds
        .iter()
        .copied()
        .find(|at| at.threshold == threshold)
        .expect("a row")
}

/// Each pick counts at every threshold it reaches, agreeing or wrong; a
/// question Gate is not measured, and the stand-in reads the owner's
/// earlier answers to the entry.
#[tokio::test]
async fn picks_count_as_agreed_or_wrong_at_each_threshold_they_reach() {
    let gates = [
        plan_gate("approve", Some("Looks right, go ahead."), GateKind::Flow),
        plan_gate("revise", Some("It needs tests first."), GateKind::Flow),
        plan_gate("answer", Some("Use cookies."), GateKind::Question),
    ];
    let model = FakeGateModel::default();
    // In order: the first Gate's reply, then its stand-in, then the
    // second's reply and stand-in.
    model.then(Some("approve"), 0.9);
    model.then(Some("approve"), 0.95);
    model.then(Some("approve"), 0.8);
    model.then(Some("revise"), 0.9);

    let report = replay(&model, &gates).await;

    assert_eq!(report.replies.gates, 2);
    assert_eq!(
        row(&report.replies, 80),
        AtThreshold {
            threshold: 80,
            decided: 2,
            agreed: 1,
            wrong: 1
        }
    );
    assert_eq!(
        row(&report.replies, 85),
        AtThreshold {
            threshold: 85,
            decided: 1,
            agreed: 1,
            wrong: 0
        }
    );
    assert_eq!(row(&report.stand_in, 90).agreed, 2);
    assert_eq!(row(&report.stand_in, 95).decided, 1);
    let asked = model.asked();
    assert_eq!(asked.len(), 4);
    assert!(asked[3].prompt.contains("Looks right, go ahead."));
    // The stand-in is asked with the Run's plan, as in a Run.
    assert!(asked[1].prompt.contains("Change the session check."));
    // Too few Gates set no threshold.
    assert_eq!(report.replies.threshold(), None);
    assert!(describe(&report).contains("Too few gates"));
}

/// With enough Gates, the threshold to set is the lowest with no wrong
/// pick; a failed call decides nothing.
#[tokio::test]
async fn the_threshold_is_the_lowest_with_no_wrong_pick() {
    let gates: Vec<AnsweredGate> = (0..ENOUGH_GATES)
        .map(|_| plan_gate("approve", Some("Go ahead"), GateKind::Flow))
        .collect();
    let model = FakeGateModel::default();
    for index in 0..ENOUGH_GATES {
        // A wrong pick at 75%, and right ones at 90%; the stand-in's call
        // between them fails.
        if index == 0 {
            model.then(Some("revise"), 0.75);
        } else {
            model.then(Some("approve"), 0.9);
        }
        model
            .picks
            .lock()
            .expect("picks")
            .push_back(Err(crate::domain::fakes::FakeError));
    }

    let report = replay(&model, &gates).await;

    assert_eq!(report.replies.threshold(), Some(80));
    assert_eq!(report.stand_in.failed, ENOUGH_GATES);
    assert_eq!(report.stand_in.threshold(), None);
    // A replay's usage is no Run's: every call names none.
    let runs = model.runs.lock().expect("runs").clone();
    assert_eq!(runs.len(), 2 * ENOUGH_GATES);
    assert!(runs.iter().all(Option::is_none), "{runs:?}");
}

/// Only a built-in Flow's plan and Blockers Gates are measured for the
/// stand-in: never its merge Gate, nor a Gate of an unknown Flow.
#[test]
fn the_stand_in_is_measured_on_routine_gates_only() {
    let plan = plan_gate("approve", None, GateKind::Flow);
    let merge = AnsweredGate {
        entry: EntryId::try_from("merge-gate".to_string()).expect("valid"),
        ..plan.clone()
    };
    let unknown = AnsweredGate {
        flow: None,
        ..plan.clone()
    };

    assert_eq!(
        super::stand_in_kind(&plan),
        Some(factory_records::domain::standing_rules::RuleGateKind::Plan)
    );
    assert_eq!(super::stand_in_kind(&merge), None);
    assert_eq!(super::stand_in_kind(&unknown), None);
}

/// A Gate's kind is its Run's own Flow's, as the Run kept it: an entry
/// `plan` that asks to merge is never measured for the stand-in.
#[test]
fn the_stand_in_reads_the_kind_of_the_run_s_own_flow() {
    let kept = serde_json::json!({
        "entries": [{
            "id": "plan",
            "step": "ready_to_merge",
            "gate": { "kind": "merge", "question": "Merge?", "answers": ["approve", "revise"] },
            "on": {}
        }]
    });
    let merging = AnsweredGate {
        flow_definition: Some(kept),
        ..plan_gate("approve", None, GateKind::Flow)
    };

    assert_eq!(super::stand_in_kind(&merging), None);
}

/// A threshold with no wrong pick is set only from enough Gates the model
/// answered at it.
#[tokio::test]
async fn a_threshold_needs_enough_gates_answered_at_it() {
    let gates: Vec<AnsweredGate> = (0..ENOUGH_GATES)
        .map(|_| plan_gate("approve", Some("Go ahead"), GateKind::Flow))
        .collect();
    let model = FakeGateModel::default();
    for index in 0..ENOUGH_GATES {
        if index < ENOUGH_DECIDED - 1 {
            model.then(Some("approve"), 0.95);
        } else {
            model.then(None, 0.95);
        }
        model.then(None, 0.95);
    }

    let report = replay(&model, &gates).await;

    assert_eq!(row(&report.replies, 95).decided, ENOUGH_DECIDED - 1);
    assert_eq!(row(&report.replies, 95).wrong, 0);
    assert_eq!(report.replies.threshold(), None);
}
