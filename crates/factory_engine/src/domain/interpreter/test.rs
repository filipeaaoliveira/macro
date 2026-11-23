use factory_records::domain::models::{Outcome, RunOutcome};

use super::{Interpreter, InterpreterError, NextStep};
use crate::domain::built_in_steps::past_merge_outcome;
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
            "accept#1",
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
            "pass",
            "published",
            "red",
            "done",
            "pass",
            "pass",
            "published",
            "green",
            "approve",
            "merged",
        ],
    );

    // The review had used two visits; the reset lets it, and the
    // acceptance after it, run from #1 again.
    assert_eq!(
        steps,
        [
            "plan#1",
            "write#1",
            "review#1",
            "fix#1",
            "review#2",
            "accept#1",
            "publish#1",
            "ci#1",
            "ci-fix#1",
            "review#1",
            "accept#1",
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
    let mut outcomes = vec!["approve", "done", "pass", "pass", "published", "red"];
    for _ in 0..3 {
        outcomes.extend(["done", "pass", "pass", "published", "red"]);
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
            "pass",
            "published",
            "green",
            "revise",
            "done",
            "pass",
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
            "accept#1",
            "publish#1",
            "ci#1",
            "merge-gate#1",
            "fix#1",
            "review#1",
            "accept#1",
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
        "off",
        "published",
        "green",
    ];
    for _ in 0..3 {
        outcomes.extend(["revise", "done", "pass", "off", "published", "green"]);
    }

    let (steps, end) = walk(&flow(CODE_FLOW), &outcomes);

    // After three reviews, the first revise is still reviewed: the reset
    // counts its visits from one.
    assert_eq!(steps[10..13], ["merge-gate#1", "fix#3", "review#1"]);
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
        steps[6..],
        // Each approval starts `merge`'s visits over.
        ["merge-gate#1", "merge#1", "ci#2", "merge-gate#2", "merge#1"]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

#[test]
fn nothing_to_write_or_a_refused_pull_request_is_left_for_a_person() {
    let (unchanged, unchanged_end) = walk(&flow(CODE_FLOW), &["approve", "unchanged"]);
    let (refused, refused_end) = walk(
        &flow(CODE_FLOW),
        &["approve", "done", "pass", "pass", "refused"],
    );

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

/// A merge held by a red base goes back to the merge Gate, whose visits
/// start over, so a Run whose Tampered merges went back through `ci`
/// before is still asked again, and merges once approved.
#[test]
fn a_held_merge_asks_again_even_after_tampered_merges_went_back() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "pass",
            "pass",
            "published",
            "green",
            "approve",
            "changed",
            "green",
            "approve",
            "changed",
            "green",
            "approve",
            "held",
            "approve",
            "merged",
        ],
    );

    assert_eq!(
        steps[6..],
        [
            "merge-gate#1",
            "merge#1",
            "ci#2",
            "merge-gate#2",
            "merge#1",
            "ci#3",
            "merge-gate#3",
            "merge#1",
            "merge-gate#1",
            "merge#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

/// A branch behind its base is updated and goes through CI to the merge
/// Gate again; one that waited out its turn in the queue goes on waiting
/// with its approval, asking no one; one whose base moved at the merge is
/// updated, and a conflict a person had to resolve, or an agent resolved,
/// is reviewed and accepted again before the merge Gate asks again.
#[test]
fn a_branch_behind_its_base_is_updated_and_checked_before_it_merges() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "pass",
            "pass",
            "published",
            "green",
            "behind",
            "done",
            "green",
            "approve",
            "queued",
            "behind",
            "retry",
            "resolved",
            "pass",
            "pass",
            "published",
            "green",
            "approve",
            "merged",
        ],
    );

    assert_eq!(
        steps[6..],
        [
            "merge-gate#1",
            "update#1",
            "ci#2",
            "merge-gate#1",
            "merge#1",
            "merge#2",
            "update#2",
            "update#3",
            "review#1",
            "accept#1",
            "publish#2",
            "ci#3",
            "merge-gate#1",
            "merge#1",
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));
}

/// A Run whose turn in its base's queue keeps not coming waits on with the
/// approval it has, asking no one, for twelve waits; then its merge Gate
/// asks again, and a new approval gives it twelve more. Three askings of
/// the Gate (some fifteen hours in the queue) end it for a person.
#[test]
fn a_queued_merge_keeps_its_approval_and_asks_again_after_twelve_waits() {
    let mut outcomes = vec![
        "approve",
        "done",
        "pass",
        "pass",
        "published",
        "green",
        "approve",
    ];
    outcomes.extend(["queued"; 12]);
    outcomes.push("approve");
    outcomes.extend(["queued"; 11]);
    outcomes.push("merged");
    let (steps, end) = walk(&flow(CODE_FLOW), &outcomes);

    let waits: Vec<String> = (1..=12).map(|visit| format!("merge#{visit}")).collect();
    assert_eq!(steps[6], "merge-gate#1");
    assert_eq!(steps[7..19], waits[..]);
    assert_eq!(steps[19], "merge-gate#2", "asked again after twelve waits");
    assert_eq!(steps[20..32], waits[..]);
    assert_eq!(end, Ok(RunOutcome::Success));

    let mut endless = vec!["approve", "done", "pass", "pass", "published", "green"];
    for _ in 0..3 {
        endless.push("approve");
        endless.extend(["queued"; 12]);
    }
    let (steps, end) = walk(&flow(CODE_FLOW), &endless);
    assert_eq!(steps.last().map(String::as_str), Some("merge#12"));
    assert_eq!(end, Ok(RunOutcome::Manual));
}

/// A conflict no one resolves ends the Run for a person; so do updates
/// past their ten visits.
#[test]
fn a_conflict_left_alone_or_endless_updates_end_for_a_person() {
    let (_, stopped) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "pass",
            "pass",
            "published",
            "green",
            "behind",
            "stop",
        ],
    );
    assert_eq!(stopped, Ok(RunOutcome::Manual));

    let mut retries = vec![
        "approve",
        "done",
        "pass",
        "pass",
        "published",
        "green",
        "behind",
    ];
    retries.extend(["retry"; 10]);
    let (steps, endless) = walk(&flow(CODE_FLOW), &retries);
    assert_eq!(steps.last().map(String::as_str), Some("update#10"));
    assert_eq!(endless, Ok(RunOutcome::Manual));
}

/// A base people keep merging into leaves the merge Gate's visits alone:
/// each `behind` starts them over, so its three visits bound only how
/// often a person sends the change back or its head moves. The updates'
/// own ten visits bound a base that moves faster than the Run's CI.
#[test]
fn a_base_that_keeps_moving_never_exhausts_the_merge_gate() {
    let mut outcomes = vec!["approve", "done", "pass", "pass", "published", "green"];
    for _ in 0..4 {
        outcomes.extend(["behind", "done", "green"]);
    }
    outcomes.extend(["approve", "merged"]);
    let (steps, end) = walk(&flow(CODE_FLOW), &outcomes);
    assert_eq!(end, Ok(RunOutcome::Success));
    assert_eq!(
        steps
            .iter()
            .rfind(|step| step.starts_with("merge-gate"))
            .map(String::as_str),
        Some("merge-gate#1")
    );

    let mut chasing = vec!["approve", "done", "pass", "pass", "published", "green"];
    for _ in 0..10 {
        chasing.extend(["behind", "done", "green"]);
    }
    chasing.push("behind");
    let (steps, end) = walk(&flow(CODE_FLOW), &chasing);
    assert_eq!(steps.last().map(String::as_str), Some("merge-gate#1"));
    assert_eq!(
        end,
        Ok(RunOutcome::Manual),
        "the eleventh update is past the bound"
    );
}

/// A review panel whose blockers stay unresolved, or whose review failed,
/// leaves the branch for a person.
#[test]
fn an_unresolved_or_failed_review_is_left_for_a_person() {
    for outcome in ["unresolved", "failed"] {
        let (steps, end) = walk(&flow(CODE_FLOW), &["approve", "done", outcome]);

        assert_eq!(steps, ["plan#1", "write#1", "review#1"], "{outcome}");
        assert_eq!(end, Ok(RunOutcome::Manual), "{outcome}");
    }
}

/// A Step that goes on after its agent's question takes the next place as
/// the same visit of the entry, which then routes the outcome it ends
/// with; a Run that took its most Steps has none left and ends `blocked`.
#[test]
fn a_step_going_on_after_a_question_takes_the_next_place_as_the_same_visit() {
    let flow = flow(REPLY_FLOW);
    let interpreter = Interpreter::new(&flow);
    let mut state = interpreter.start();
    let NextStep::Run { seq, visit, .. } = interpreter.next_step(&mut state).expect("next") else {
        panic!("the reply flow runs a step");
    };
    assert_eq!((seq, visit), (1, 1));

    assert_eq!(interpreter.resume(&mut state), Ok(Some(2)));
    assert_eq!(interpreter.resume(&mut state), Ok(Some(3)));
    interpreter
        .record(&mut state, &Outcome::done())
        .expect("record");
    assert_eq!(
        interpreter.next_step(&mut state),
        Ok(NextStep::End(RunOutcome::Success))
    );
    assert_eq!(interpreter.resume(&mut state), Err(InterpreterError::Ended));

    let mut full = interpreter.start();
    interpreter.next_step(&mut full).expect("next");
    for _ in 1..factory_records::domain::models::MAX_STEPS {
        interpreter.resume(&mut full).expect("resume");
    }
    assert_eq!(interpreter.resume(&mut full), Ok(None));
    assert_eq!(
        interpreter.next_step(&mut full),
        Ok(NextStep::End(RunOutcome::Blocked))
    );
}

/// A Flow whose agents may ask questions says how many; one written before
/// questions reads with none, and writes no `questions`, so the input of a
/// Run in flight serializes as it did.
#[test]
fn a_flow_without_questions_reads_and_writes_as_before() {
    let old = flow(SINGLE_REVIEWER_CODE_FLOW);
    assert_eq!(old.questions, None);
    let written = serde_json::to_value(&old).expect("serializes");
    assert!(written.get("questions").is_none());

    let mut asking = old.clone();
    asking.allow_questions(2);
    let written = serde_json::to_value(&asking).expect("serializes");
    assert_eq!(written["questions"], 2);
    let read: Flow = serde_json::from_value(written).expect("reads back");
    assert_eq!(read.questions, Some(2));
}

/// Before the merge, a verifier checks the change once the review passed:
/// its Blockers Gate's `fix` sends it to a fix, through the review again
/// from its first visit, and it is checked again. Its last visit's failure
/// routes `unresolved`, which leaves the branch for a person with no fix
/// after it; so does `stop`. Its failures never use up review rounds.
#[test]
fn acceptance_that_fails_before_the_merge_goes_back_to_be_fixed() {
    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "pass",
            "fix",
            "done",
            "pass",
            "pass",
            "published",
            "green",
            "approve",
            "merged",
        ],
    );
    assert_eq!(
        steps[..9],
        [
            "plan#1",
            "write#1",
            "review#1",
            "accept#1",
            "fix#1",
            "review#1",
            "accept#2",
            "publish#1",
            "ci#1"
        ]
    );
    assert_eq!(end, Ok(RunOutcome::Success));

    let (steps, end) = walk(
        &flow(CODE_FLOW),
        &[
            "approve",
            "done",
            "changes",
            "done",
            "changes",
            "done",
            "pass",
            "fix",
            "done",
            "pass",
            "fix",
            "done",
            "pass",
            "unresolved",
        ],
    );
    assert_eq!(steps.last().map(String::as_str), Some("accept#3"));
    assert!(steps.iter().all(|step| step != "fix#5"), "{steps:?}");
    assert_eq!(end, Ok(RunOutcome::Manual));

    let (steps, end) = walk(&flow(CODE_FLOW), &["approve", "done", "pass", "stop"]);
    assert_eq!(steps.last().map(String::as_str), Some("accept#1"));
    assert_eq!(end, Ok(RunOutcome::Manual));
}

/// The Run ends at its merge: one `merge` made, or one `ready_to_merge`
/// found, ends it `success`, and no Step runs after it.
#[test]
fn a_run_ends_once_its_change_is_merged() {
    let code = flow(CODE_FLOW);
    assert!(
        code.entries
            .iter()
            .all(|entry| !["deploy", "verify"].contains(&entry.step.as_str()))
    );
    let to_merge_gate = ["approve", "done", "pass", "pass", "published", "green"];

    let mut merged = to_merge_gate.to_vec();
    merged.extend(["approve", "merged"]);
    let (steps, end) = walk(&code, &merged);
    assert_eq!(steps.last().map(String::as_str), Some("merge#1"));
    assert_eq!(end, Ok(RunOutcome::Success));

    let mut found = to_merge_gate.to_vec();
    found.push("merged");
    let (steps, end) = walk(&code, &found);
    assert_eq!(steps.last().map(String::as_str), Some("merge-gate#1"));
    assert_eq!(end, Ok(RunOutcome::Success));
}

/// The `code` Flows as Runs in flight carry them, from before the Run ended
/// at its merge: once merged, they go on to `deploy` and `verify`, whose
/// Steps now end at once (`built_in_steps::past_merge_outcome`), which ends
/// the Run `success` at its next Step, wherever it stood after the merge.
/// The first of them (`first_deploy_verify_code.json`) has no `off` from
/// `verify`; the later one (`deploy_verify_code.json`) has.
const DEPLOY_VERIFY_CODE_FLOWS: [(&str, &str); 2] = [
    (
        "first_deploy_verify_code",
        include_str!("first_deploy_verify_code.json"),
    ),
    (
        "deploy_verify_code",
        include_str!("deploy_verify_code.json"),
    ),
];

#[test]
fn a_run_in_flight_past_its_merge_ends_at_its_next_step() {
    let ended = |step: &str| past_merge_outcome(step).expect("served");
    let merged = [
        "approve",
        "done",
        "pass",
        "pass",
        "published",
        "green",
        "approve",
        "merged",
    ];
    for (name, definition) in DEPLOY_VERIFY_CODE_FLOWS {
        let old = flow(definition);
        let after = |outcomes: &[&str]| {
            let mut all = merged.to_vec();
            all.extend_from_slice(outcomes);
            let (steps, end) = walk(&old, &all);
            (steps[merged.len()..].to_vec(), end)
        };

        // One merged on a worker without the Steps: `deploy` ends it.
        assert_eq!(
            after(&[ended("deploy")]),
            (vec!["deploy#1".to_string()], Ok(RunOutcome::Success)),
            "{name}"
        );
        // One whose deploy was waited for already: `verify` ends it.
        for waited in ["deployed", "not_needed"] {
            assert_eq!(
                after(&[waited, ended("verify")]),
                (
                    vec!["deploy#1".to_string(), "verify#1".to_string()],
                    Ok(RunOutcome::Success)
                ),
                "{name}: {waited}"
            );
        }
    }
}

/// The `code` Flow as Runs in flight carry it, from before the verify
/// Phase: their workflow's input holds it, so they keep walking it and end
/// at their merge, never reaching a Step a worker added since.
const REVIEW_PANEL_CODE_FLOW: &str = include_str!("review_panel_code.json");

#[test]
fn a_run_in_flight_on_the_code_flow_before_verify_ends_at_its_merge() {
    let old = flow(REVIEW_PANEL_CODE_FLOW);
    assert!(
        old.entries
            .iter()
            .all(|entry| !["accept", "deploy", "verify"].contains(&entry.step.as_str()))
    );

    let (steps, end) = walk(
        &old,
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

    assert_eq!(steps.last().map(String::as_str), Some("merge#1"));
    assert_eq!(end, Ok(RunOutcome::Success));
}

/// A retry starts at the entry the Run it retries stopped at, its first
/// Step at the place past the Steps it kept, and walks on as the Flow
/// routes from there: a `code` Run that stopped at `write` does not plan
/// again.
#[test]
fn a_retry_starts_at_its_entry_past_the_steps_it_kept() {
    let code = flow(CODE_FLOW);
    let interpreter = Interpreter::new(&code);
    let write = "write".to_string().try_into().expect("valid entry");
    let mut state = interpreter.start_at(&write, 3).expect("start");

    let NextStep::Run { entry, visit, seq } = interpreter.next_step(&mut state).expect("next")
    else {
        panic!("the retry runs a step");
    };
    assert_eq!((entry.id.as_str(), visit, seq), ("write", 1, 3));
    interpreter
        .record(&mut state, &Outcome::done())
        .expect("record");
    let NextStep::Run { entry, seq, .. } = interpreter.next_step(&mut state).expect("next") else {
        panic!("the retry goes on");
    };
    assert_eq!((entry.id.as_str(), seq), ("review", 4));
}

/// A retry may start only at an entry its Flow has.
#[test]
fn a_retry_at_an_entry_its_flow_lacks_cannot_start() {
    let code = flow(CODE_FLOW);
    let interpreter = Interpreter::new(&code);
    let gone = "deploy".to_string().try_into().expect("valid entry");

    assert_eq!(
        interpreter.start_at(&gone, 2),
        Err(InterpreterError::UnknownEntry(gone))
    );
}

/// A retry takes as many Steps of its own as a new Run, however many it
/// kept: a Run that stopped at its most Steps is retried with room to go
/// on.
#[test]
fn a_retry_takes_its_most_steps_past_the_ones_it_kept() {
    let looping = flow(
        r#"{
            "entries": [
                { "id": "a", "step": "work",
                  "on": { "done": [{ "to": { "kind": "entry", "entry": "a" } }] } }
            ]
        }"#,
    );
    let interpreter = Interpreter::new(&looping);
    let entry = "a".to_string().try_into().expect("valid entry");
    let kept = factory_records::domain::models::MAX_STEPS;
    let mut state = interpreter.start_at(&entry, kept + 1).expect("start");
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
    assert_eq!(places.len(), usize::from(kept));
    assert_eq!(places.first(), Some(&(kept + 1)));
    assert_eq!(places.last(), Some(&(2 * kept)));
}
