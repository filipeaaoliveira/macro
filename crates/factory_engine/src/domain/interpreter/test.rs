use factory_records::domain::models::{Outcome, RunOutcome};

use super::{Interpreter, InterpreterError, NextStep};
use crate::domain::flow::{FactName, Flow};

const REPLY_FLOW: &str = include_str!("../../../flows/reply.json");
const CODE_FLOW: &str = include_str!("../../../flows/code.json");

fn flow(definition: &str) -> Flow {
    let definition = serde_json::from_str(definition).expect("the flow is JSON");
    Flow::parse(definition, |_| true).expect("the flow holds together")
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
            Ok(NextStep::Run { entry, visit }) => {
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

#[test]
fn a_clean_review_goes_straight_to_merge() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &["done", "done", "clean", "published", "green", "merged"],
    );

    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "review#1",
            "publish#1",
            "ci#1",
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

    let NextStep::Run { entry, visit } = interpreter.next_step(&mut state).expect("next") else {
        panic!("the run ended after plan");
    };
    assert_eq!((entry.id.as_str(), visit), ("write", 1));
}

#[test]
fn blockers_loop_through_fix_until_the_review_runs_out_of_visits() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "done",
            "done",
            "blockers",
            "done",
            "blockers",
            "done",
            "published",
            "green",
        ],
    );

    // The third review is past max_visits: on_exhausted holds the pull
    // request, so publish and CI run, and merge is skipped for a person.
    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "review#1",
            "fix#1",
            "review#2",
            "fix#2",
            "publish#1",
            "ci#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Manual));
}

#[test]
fn a_transition_sets_the_facts_later_conditions_read() {
    let code = flow(CODE_FLOW);
    let interpreter = Interpreter::new(&code);
    let mut state = interpreter.start();
    let panel_ran = FactName::try_from("panel_ran".to_string()).expect("valid");
    assert!(!state.fact(&panel_ran));

    for outcome in ["done", "done", "clean"] {
        interpreter.next_step(&mut state).expect("a step runs");
        let outcome = Outcome::try_from(outcome.to_string()).expect("valid");
        interpreter.record(&mut state, &outcome).expect("a way on");
    }

    assert!(state.fact(&panel_ran));
}

#[test]
fn a_ci_fix_after_a_review_resets_the_reviews_visits() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "done",
            "done",
            "blockers",
            "done",
            "clean",
            "published",
            "red",
            "done",
            "clean",
            "published",
            "green",
            "merged",
        ],
    );

    // The review had used both visits; the reset lets it run from #1 again.
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
            "merge#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

#[test]
fn a_ci_fix_before_any_review_goes_back_to_publish() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "done",
            "unchanged",
            "published",
            "red",
            "done",
            "published",
            "green",
            "merged",
        ],
    );

    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "publish#1",
            "ci#1",
            "ci-fix#1",
            "publish#2",
            "ci#2",
            "merge#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
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
    let (steps, end) = walk(&flow(CODE_FLOW), &["done", "failed"]);

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
