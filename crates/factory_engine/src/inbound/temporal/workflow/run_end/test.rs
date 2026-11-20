use factory_records::domain::models::{Outcome, RunOutcome};
use serde_json::json;

use super::RunEnding;
use crate::domain::flow::{Flow, StepUse};
use crate::domain::interpreter::{Interpreter, NextStep, RunState};
use crate::inbound::temporal::workflow::review_round::Round;

/// A Flow of `entries`, each an agent's Step.
fn flow(entries: serde_json::Value) -> Flow {
    Flow::parse(json!({ "entries": entries }), |_| Some(StepUse::Agent)).expect("valid flow")
}

/// Walk `flow`'s Steps with `rounds`, in order, noting each in a
/// `RunEnding` as the workflow does, and return it with how the Run ended.
fn walk(flow: &Flow, rounds: Vec<Round>) -> (RunEnding, RunOutcome) {
    let interpreter = Interpreter::new(flow);
    let mut state: RunState = interpreter.start();
    let mut ending = RunEnding::default();
    for round in rounds {
        let NextStep::Run { .. } = interpreter.next_step(&mut state).expect("next") else {
            panic!("the run ended before its rounds");
        };
        let routed = match &round {
            Round::Routed(outcome) => outcome.clone(),
            Round::FailedForInfrastructure => Outcome::failed(),
            Round::Ended(_) => unreachable!("no round here ends the run"),
        };
        interpreter.record(&mut state, &routed).expect("record");
        ending.routed(&round, &state);
    }
    let NextStep::End(outcome) = interpreter.next_step(&mut state).expect("next") else {
        panic!("the run did not end");
    };
    (ending, outcome)
}

#[test]
fn a_run_failed_on_work_that_failed_for_infrastructure_ends_so() {
    let plan = flow(json!([{ "id": "plan", "step": "plan", "role": "planner" }]));

    let (ending, outcome) = walk(&plan, vec![Round::FailedForInfrastructure]);

    assert_eq!(outcome, RunOutcome::Failed);
    assert!(ending.infrastructure(outcome, false));
    assert!(!ending.infrastructure(RunOutcome::Blocked, false));
}

/// Work that failed for infrastructure, routed on to a later Step whose
/// own work then fails, ends the Run as that later failure does.
#[test]
fn a_later_failure_of_the_work_ends_the_run_failed() {
    let plan_then_fix = flow(json!([
        {
            "id": "plan", "step": "plan", "role": "planner",
            "on": { "failed": [{ "to": { "kind": "entry", "entry": "fix" } }] }
        },
        { "id": "fix", "step": "fix", "role": "fixer" }
    ]));

    let (ending, outcome) = walk(
        &plan_then_fix,
        vec![
            Round::FailedForInfrastructure,
            Round::Routed(Outcome::failed()),
        ],
    );

    assert_eq!(outcome, RunOutcome::Failed);
    assert!(!ending.infrastructure(outcome, false));
}

/// A Flow that could not go on fails the Run for itself, whatever the
/// last round was.
#[test]
fn a_flow_that_stopped_ends_the_run_failed() {
    let plan = flow(json!([{ "id": "plan", "step": "plan", "role": "planner" }]));

    let (ending, outcome) = walk(&plan, vec![Round::FailedForInfrastructure]);

    assert!(!ending.infrastructure(outcome, true));
}

/// A `failed` of infrastructure's that takes the Run back to an entry out
/// of visits, which ends it `failed`, is the work's: the loop did not
/// converge.
#[test]
fn a_run_ended_failed_on_exhausted_visits_is_the_work_s() {
    let retrying = flow(json!([{
        "id": "plan", "step": "plan", "role": "planner",
        "max_visits": 1,
        "on": { "failed": [{ "to": { "kind": "entry", "entry": "plan" } }] },
        "on_exhausted": [{ "to": { "kind": "end", "outcome": "failed" } }]
    }]));

    let (ending, outcome) = walk(&retrying, vec![Round::FailedForInfrastructure]);

    assert_eq!(outcome, RunOutcome::Failed);
    assert!(!ending.infrastructure(outcome, false));
}
