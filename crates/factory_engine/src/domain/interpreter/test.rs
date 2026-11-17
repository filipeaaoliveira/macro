use factory_records::domain::models::{Outcome, RunOutcome};

use super::{Interpreter, InterpreterError, NextStep};
use crate::domain::flow::{FactName, Flow};

const REPLY_FLOW: &str = include_str!("../../../flows/reply.json");
const CODE_FLOW: &str = include_str!("../../../flows/code.json");

fn flow(definition: &str) -> Flow {
    let definition = serde_json::from_str(definition).expect("the flow is JSON");
    Flow::parse(definition, |_| Some(crate::domain::flow::StepUse::Work))
        .expect("the flow holds together")
}

/// The Steps a Run of `flow` takes when its Steps end with `outcomes`, in
/// order, as `entry#visit`, and how the Run ends.
fn walk(flow: &Flow, outcomes: &[&str]) -> (Vec<String>, Result<RunOutcome, InterpreterError>) {
    let interpreter = Interpreter::new(flow);
    let mut state = interpreter.start();
    let mut steps = Vec::new();
    let mut outcomes = outcomes.iter();
    loop {
        match interpreter.next_step(&mut state) {
            Ok(NextStep::End(end)) => return (steps, Ok(end)),
            Err(error) => return (steps, Err(error)),
            Ok(NextStep::Run { entry, visit, .. }) => {
                steps.push(format!("{}#{visit}", entry.id));
                let next = outcomes
                    .next()
                    .unwrap_or_else(|| panic!("{} ran with no outcome left", entry.id));
                let outcome = Outcome::try_from(next.to_string()).expect("valid outcome");
                if let Err(error) = interpreter.record(&mut state, &outcome) {
                    return (steps, Err(error));
                }
            }
        }
    }
}

#[test]
fn the_reply_flow_runs_one_step_and_succeeds() {
    let (steps, end) = walk(&flow(REPLY_FLOW), &["done"]);

    assert_eq!(steps, ["reply#1"]);
    assert_eq!(end, Ok(RunOutcome::Success));
}

// A Gate's answer is the outcome its entry routes, so `approve` and `revise`
// stand for a person's answer to the plan and the merge question below.

#[test]
fn a_passing_review_goes_through_ci_to_the_merge_question() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "pass",
            "published",
            "green",
            "approve",
            "merged",
        ],
    );

    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "review#1",
            "publish#1",
            "ci#1",
            "merge-gate#1",
            "merge#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

#[test]
fn an_outcome_without_a_transition_goes_to_the_entry_below() {
    let code = flow(CODE_FLOW);
    let interpreter = Interpreter::new(&code);
    let mut state = interpreter.start();

    interpreter.next_step(&mut state).expect("plan runs");
    interpreter
        .record(
            &mut state,
            &Outcome::try_from("done".to_string()).expect("valid"),
        )
        .expect("plan has a way on");

    let NextStep::Run { entry, visit, seq } = interpreter.next_step(&mut state).expect("next")
    else {
        panic!("the run ended after plan");
    };
    assert_eq!((entry.id.as_str(), visit, seq), ("write", 1, 2));
}

#[test]
fn changes_loop_through_fix_until_the_review_runs_out_of_rounds() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve", "done", "changes", "done", "changes", "done", "changes", "done",
        ],
    );

    // The fourth review is past max_visits: the Run ends for a person, as
    // red checks end it, before any pull request is opened.
    assert_eq!(
        steps,
        [
            "plan#1", "write#1", "review#1", "fix#1", "review#2", "fix#2", "review#3", "fix#3"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Manual));
}

/// A review panel takes a place for each of its reviewers, so the Step
/// after it starts past them all.
#[test]
fn a_review_panel_takes_a_place_for_each_reviewer() {
    let code = flow(CODE_FLOW);
    let interpreter = Interpreter::new(&code);
    let mut state = interpreter.start();
    let mut places = Vec::new();

    for outcome in ["approve", "done", "changes", "done"] {
        let NextStep::Run { entry, seq, .. } = interpreter.next_step(&mut state).expect("next")
        else {
            panic!("the run ended early");
        };
        places.push((entry.id.to_string(), seq));
        let outcome = Outcome::try_from(outcome.to_string()).expect("valid");
        interpreter.record(&mut state, &outcome).expect("a way on");
    }
    let NextStep::Run { entry, seq, .. } = interpreter.next_step(&mut state).expect("next") else {
        panic!("the run ended after the fix");
    };
    places.push((entry.id.to_string(), seq));

    let places: Vec<_> = places
        .iter()
        .map(|(entry, seq)| (entry.as_str(), *seq))
        .collect();
    assert_eq!(
        places,
        [
            ("plan", 1),
            ("write", 2),
            ("review", 3),
            ("fix", 6),
            ("review", 7)
        ]
    );
}

/// Checks that never go green end the Run for a person, from `write` and
/// from `fix` alike, before any pull request is opened.
#[test]
fn red_checks_end_the_code_run_for_a_person() {
    let (steps, end) = walk(&flow(CODE_FLOW), &["approve", "red"]);
    assert_eq!(steps, ["plan#1", "write#1"]);
    assert_eq!(end, Ok(RunOutcome::Manual));

    let (steps, end) = walk(&flow(CODE_FLOW), &["approve", "done", "changes", "red"]);
    assert_eq!(steps, ["plan#1", "write#1", "review#1", "fix#1"]);
    assert_eq!(end, Ok(RunOutcome::Manual));
}

#[test]
fn a_transition_sets_the_facts_later_conditions_read() {
    let held_flow = flow(
        r#"{
            "facts": { "held": false },
            "entries": [
                { "id": "review", "step": "review", "max_visits": 1,
                  "on_exhausted": [{ "to": { "kind": "entry", "entry": "publish" }, "set": { "held": true } }],
                  "on": { "changes": [{ "to": { "kind": "entry", "entry": "review" } }] } },
                { "id": "publish", "step": "publish",
                  "on": { "done": [
                      { "when": [{ "fact": "held", "equals": true }], "to": { "kind": "end", "outcome": "manual" } },
                      { "to": { "kind": "end", "outcome": "success" } }
                  ] } }
            ]
        }"#,
    );
    let interpreter = Interpreter::new(&held_flow);
    let mut state = interpreter.start();
    let held = FactName::try_from("held".to_string()).expect("valid");
    assert!(!state.fact(&held));

    interpreter.next_step(&mut state).expect("the review runs");
    let changes = Outcome::try_from("changes".to_string()).expect("valid");
    interpreter.record(&mut state, &changes).expect("a way on");
    let NextStep::Run { entry, .. } = interpreter.next_step(&mut state).expect("next") else {
        panic!("the run ended after the review");
    };

    assert_eq!(entry.id.as_str(), "publish");
    assert!(state.fact(&held));
    interpreter
        .record(&mut state, &Outcome::done())
        .expect("a way on");
    assert_eq!(
        interpreter.next_step(&mut state),
        Ok(NextStep::End(RunOutcome::Manual))
    );
}

#[test]
fn red_ci_is_fixed_reviewed_again_and_goes_green() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "changes",
            "done",
            "pass",
            "published",
            "red",
            "done",
            "pass",
            "published",
            "green",
            "approve",
            "merged",
        ],
    );

    // The review had used two visits; the reset lets it run from #1 again.
    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "review#1",
            "fix#1",
            "review#2",
            "publish#1",
            "ci#1",
            "ci-fix#1",
            "review#1",
            "publish#2",
            "ci#2",
            "merge-gate#1",
            "merge#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

#[test]
fn ci_that_stays_red_after_three_fixes_is_left_for_a_person() {
    let mut outcomes = vec!["approve", "done", "pass", "published", "red"];
    for _ in 0..3 {
        outcomes.extend(["done", "pass", "published", "red"]);
    }

    let (steps, end) = walk(&flow(CODE_FLOW), &outcomes);

    assert_eq!(
        steps
            .iter()
            .filter(|step| step.starts_with("ci-fix#"))
            .count(),
        3
    );
    assert_eq!(steps.last().map(String::as_str), Some("ci#4"));
    assert_eq!(end, Ok(RunOutcome::Manual));
}

#[test]
fn a_person_sending_the_merge_back_has_the_change_fixed_and_asked_again() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "pass",
            "published",
            "green",
            "revise",
            "done",
            "pass",
            "published",
            "green",
            "approve",
            "merged",
        ],
    );

    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "review#1",
            "publish#1",
            "ci#1",
            "merge-gate#1",
            "fix#1",
            "review#1",
            "publish#2",
            "ci#2",
            "merge-gate#2",
            "merge#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

/// A person's revise is reviewed again even after the reviews ran out of
/// visits, and the merge question is asked at most three times.
#[test]
fn a_revise_at_the_merge_question_is_reviewed_again_and_the_question_is_capped() {
    let mut outcomes = vec![
        "approve",
        "done",
        "changes",
        "done",
        "changes",
        "done",
        "pass",
        "published",
        "green",
    ];
    for _ in 0..3 {
        outcomes.extend(["revise", "done", "pass", "published", "green"]);
    }

    let (steps, end) = walk(&flow(CODE_FLOW), &outcomes);

    // After three reviews, the first revise is still reviewed: the reset
    // counts its visits from one.
    assert_eq!(steps[9..12], ["merge-gate#1", "fix#3", "review#1"]);
    assert_eq!(
        steps
            .iter()
            .filter(|step| step.starts_with("merge-gate#"))
            .count(),
        3
    );
    assert_eq!(end, Ok(RunOutcome::Manual));
}

#[test]
fn a_head_that_moved_before_the_merge_goes_back_to_ci() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "pass",
            "published",
            "green",
            "approve",
            "changed",
            "green",
            "approve",
            "merged",
        ],
    );

    assert_eq!(
        steps[5..],
        ["merge-gate#1", "merge#1", "ci#2", "merge-gate#2", "merge#2"]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

#[test]
fn nothing_to_write_or_a_refused_pull_request_is_left_for_a_person() {
    let (unchanged, unchanged_end) = walk(&flow(CODE_FLOW), &["approve", "unchanged"]);
    let (refused, refused_end) = walk(&flow(CODE_FLOW), &["approve", "done", "pass", "refused"]);

    assert_eq!(unchanged, ["plan#1", "write#1"]);
    assert_eq!(unchanged_end, Ok(RunOutcome::Manual));
    assert_eq!(refused.last().map(String::as_str), Some("publish#1"));
    assert_eq!(refused_end, Ok(RunOutcome::Manual));
}

#[test]
fn an_exhausted_entry_without_on_exhausted_ends_the_run_blocked() {
    let looping = flow(
        r#"{
            "entries": [
                {"id": "try", "step": "attempt", "max_visits": 2,
                 "on": {"again": [{"to": {"kind": "entry", "entry": "try"}}]}}
            ]
        }"#,
    );

    let (steps, end) = walk(&looping, &["again", "again"]);

    assert_eq!(steps, ["try#1", "try#2"]);
    assert_eq!(end, Ok(RunOutcome::Blocked));
}

#[test]
fn a_failed_step_the_flow_does_not_route_ends_the_run_failed() {
    let (steps, end) = walk(&flow(CODE_FLOW), &["approve", "failed"]);

    assert_eq!(steps, ["plan#1", "write#1"]);
    assert_eq!(end, Ok(RunOutcome::Failed));
}

#[test]
fn an_outcome_with_nowhere_to_go_stops_the_run() {
    let (steps, end) = walk(&flow(REPLY_FLOW), &["refused"]);

    assert_eq!(steps, ["reply#1"]);
    assert!(matches!(end, Err(InterpreterError::NoWayOn { .. })));
}

#[test]
fn a_flow_that_skips_every_entry_it_reaches_stops_instead_of_looping() {
    let skipping = flow(
        r#"{
            "facts": {"ready": false},
            "entries": [
                {"id": "first", "step": "wait", "when": [{"fact": "ready", "equals": true}],
                 "next": {"kind": "entry", "entry": "second"}},
                {"id": "second", "step": "wait", "when": [{"fact": "ready", "equals": true}],
                 "next": {"kind": "entry", "entry": "first"}}
            ]
        }"#,
    );

    let (steps, end) = walk(&skipping, &[]);

    assert!(steps.is_empty());
    assert_eq!(end, Err(InterpreterError::NoStepToRun));
}

/// A reset takes an entry's visits back to 1, so `max_visits` alone does not
/// stop a Flow that loops: the Run's cap on Steps does.
#[test]
fn a_flow_that_loops_forever_ends_blocked_at_the_most_steps() {
    let looping = flow(
        r#"{
            "entries": [
                { "id": "work", "step": "work", "max_visits": 2,
                  "on": { "done": [{ "to": { "kind": "entry", "entry": "again" } }] } },
                { "id": "again", "step": "again",
                  "on": { "done": [{ "to": { "kind": "entry", "entry": "work" }, "reset": ["work"] }] } }
            ]
        }"#,
    );
    let interpreter = Interpreter::new(&looping);
    let mut state = interpreter.start();
    let mut places = Vec::new();

    let end = loop {
        match interpreter.next_step(&mut state).expect("next") {
            NextStep::End(end) => break end,
            NextStep::Run { seq, .. } => {
                places.push(seq);
                interpreter
                    .record(&mut state, &Outcome::done())
                    .expect("record");
            }
        }
    };

    assert_eq!(end, RunOutcome::Blocked);
    assert_eq!(
        places.len(),
        usize::from(factory_records::domain::models::MAX_STEPS)
    );
    assert_eq!(
        places.last(),
        Some(&factory_records::domain::models::MAX_STEPS)
    );
}

/// The `code` Flow as Runs started before the review panel carry it: one
/// `reviewer`, and a `held` fact once its reviews run out.
const SINGLE_REVIEWER_CODE_FLOW: &str = include_str!("single_reviewer_code.json");

/// A Run started on the single-reviewer `code` Flow keeps walking it as it
/// did: it still reads, names no review panel, so the workflow takes the
/// single Step's path for every entry, and the Flow and its Steps' requests
/// serialize as they did, so their histories replay.
#[test]
fn a_run_on_the_single_reviewer_code_flow_walks_it_as_before() {
    let old = flow(SINGLE_REVIEWER_CODE_FLOW);
    assert!(old.entries.iter().all(|entry| entry.reviewers.is_empty()));

    let (steps, end) = walk(
        &old,
        &[
            "approve",
            "done",
            "changes",
            "done",
            "changes",
            "done",
            "changes",
            "done",
            "published",
            "green",
        ],
    );
    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "review#1",
            "fix#1",
            "review#2",
            "fix#2",
            "review#3",
            "fix#3",
            "publish#1",
            "ci#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Manual));

    // One place per Step, as before the panel.
    let interpreter = Interpreter::new(&old);
    let mut state = interpreter.start();
    let mut places = Vec::new();
    for outcome in ["approve", "done", "changes"] {
        let NextStep::Run { seq, entry, visit } = interpreter.next_step(&mut state).expect("next")
        else {
            panic!("the run ended early");
        };
        places.push(seq);
        let request = crate::domain::models::StepRequest::of(
            factory_records::domain::models::StepKey {
                run_id: factory_records::domain::models::RunId::generate(),
                seq,
            },
            entry,
            visit,
        );
        let written = serde_json::to_value(&request).expect("serializes");
        assert!(written.get("round").is_none(), "{written}");
        let outcome = Outcome::try_from(outcome.to_string()).expect("valid");
        interpreter.record(&mut state, &outcome).expect("a way on");
    }
    assert_eq!(places, [1, 2, 3]);

    let written = serde_json::to_value(&old).expect("serializes");
    let read: Flow = serde_json::from_value(written.clone()).expect("reads back");
    assert_eq!(read, old);
    let entries = written["entries"].as_array().expect("entries");
    assert!(entries.iter().all(|entry| entry.get("reviewers").is_none()));
}
