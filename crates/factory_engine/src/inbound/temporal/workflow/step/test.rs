//! A Step's walk over a stand-in for Temporal, which answers each activity
//! as the test scripts it and keeps what it was asked.

use std::cell::RefCell;
use std::collections::BTreeMap;

use factory_records::domain::models::{EntryId, Outcome, RunId, RunOutcome, StepKey};
use factory_records::domain::standing_rules::OrchestratorDefinition;
use serde_json::json;

use super::walk_step;
use crate::domain::flow::{Entry, Flow, StepUse};
use crate::domain::gate::GateOpening;
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::domain::standing_rules::{GateFacts, rule_answer};
use crate::inbound::temporal::workflow::review_round::Round;
use crate::inbound::temporal::workflow::work::Work;
use crate::inbound::temporal::workflow::{Asking, Question, Questions};

/// An activity that failed.
#[derive(Debug, PartialEq, Eq)]
struct Failed;

/// What the stand-in answers, and what it was asked, in order.
#[derive(Default)]
struct Script {
    /// What the Step at a place ends with, when not `done`: `question`, or
    /// `fail` for work that fails for good.
    ends: BTreeMap<u16, &'static str>,
    cancelled: bool,
    /// Asking a Gate fails, other than by the Run's cancellation.
    ask_fails: bool,
    /// The Run's Standing Rules; none for a Run created before Runs kept
    /// them.
    rules: Option<OrchestratorDefinition>,
    asked: RefCell<Vec<String>>,
    requests: RefCell<Vec<StepRequest>>,
}

impl Script {
    fn asked(&self) -> Vec<String> {
        self.asked.borrow().clone()
    }

    fn note(&self, what: String) {
        self.asked.borrow_mut().push(what);
    }
}

impl Work for Script {
    type Failure = Failed;

    fn cancelled(&self) -> bool {
        self.cancelled
    }

    async fn open(&self, _opening: ReviewRoundOpening) -> Result<bool, Failed> {
        unreachable!("a Step's walk opens no round")
    }

    async fn start(&self, request: StepRequest) -> Result<StepStart, Failed> {
        self.note(format!("start {}", request.key.seq));
        self.requests.borrow_mut().push(request);
        Ok(StepStart::Started)
    }

    async fn run(&self, request: &StepRequest) -> Result<StepReport, Failed> {
        self.note(format!("run {}", request.key.seq));
        match self.ends.get(&request.key.seq).copied() {
            Some("fail") => Err(Failed),
            Some("question") => Ok(StepReport {
                outcome: Outcome::question(),
                output: format!("Question of step {}?", request.key.seq),
            }),
            _ => Ok(StepReport {
                outcome: Outcome::done(),
                output: "The plan".to_string(),
            }),
        }
    }

    async fn run_all(&self, _requests: &[StepRequest]) -> Vec<Result<StepReport, Failed>> {
        unreachable!("a Step's walk runs one Step at a time")
    }

    async fn end(&self, ending: StepEnding) -> Result<(), Failed> {
        self.note(format!("end {} {}", ending.key.seq, ending.report.outcome));
        Ok(())
    }

    async fn release(&self, request: StepRequest) -> Result<(), Failed> {
        self.note(format!("release {}", request.key.seq));
        Ok(())
    }

    async fn stop(&self, request: &StepRequest) {
        self.note(format!("stop {}", request.key.seq));
    }

    async fn ask(&self, opening: GateOpening, rule: Option<Asking>) -> Result<Outcome, Failed> {
        // As the worker does: a Flow's Gate of a kind goes to the Run's
        // Standing Rules first, and only one they leave waits for a person.
        if let Some(asking) = rule
            && let Some(kind) = asking.kind
        {
            let facts = GateFacts {
                kind,
                fix_rounds: asking.fix_rounds,
                fix_limit: asking.fix_limit,
                tampered: false,
                held: false,
            };
            if let Some(ruled) = rule_answer(self.rules, &facts, &opening.answers) {
                self.note(format!("rule {} {}", opening.key.seq, ruled.answer));
                return Ok(ruled.answer);
            }
        }
        self.note(format!("ask {} {}", opening.key.seq, opening.question));
        if self.ask_fails {
            return Err(Failed);
        }
        Ok(opening.answers[0].clone())
    }

    async fn close(&self, _closing: ReviewRoundClosing) -> Result<Outcome, Failed> {
        unreachable!("a Step's walk closes no round")
    }
}

/// A `plan` entry, with the Gate `Approve?` when `gated`.
fn plan(gated: bool) -> Entry {
    let mut entry = json!({ "id": "plan", "step": "plan", "role": "planner" });
    if gated {
        entry["gate"] = json!({ "question": "Approve?", "answers": ["approve", "revise"] });
    }
    let flow =
        Flow::parse(json!({ "entries": [entry] }), |_| Some(StepUse::Agent)).expect("valid flow");
    flow.entries[0].clone()
}

/// The questions a Flow that lets agents ask `most` lets them ask.
fn asking(most: u8) -> Questions {
    Questions {
        may_ask: most > 0,
        most,
    }
}

/// Walk Step 1 of `entry` with `questions` on `script`, places after it
/// given from 2 while `places` last; return how it came out and the
/// places of the Steps held.
async fn walk(
    script: &Script,
    entry: &Entry,
    questions: Questions,
    places: u16,
) -> (Result<Round, Failed>, Vec<u16>) {
    let key = StepKey {
        run_id: RunId::generate(),
        seq: 1,
    };
    let request = questions.offer(StepRequest::of(key, entry, 1), 0);
    let mut held = Vec::new();
    let mut next = 2;
    let mut resume = || {
        let seq = (next <= places).then_some(next);
        next += 1;
        seq
    };
    let walked = walk_step(script, entry, request, questions, &mut held, &mut resume).await;
    (walked, held.iter().map(|request| request.key.seq).collect())
}

/// A Run on a Flow without questions walks a Step exactly as the workflow
/// did before questions: start, run, end, let go, then the entry's Gate,
/// whose answer is routed; work that failed for good has its agent stopped
/// instead. Its Step is offered no question, and one that ends `question`
/// anyway is routed as any outcome.
#[tokio::test]
async fn a_flow_without_questions_walks_a_step_as_before() {
    let none = Questions::of(
        &Flow::parse(
            json!({ "entries": [{ "id": "plan", "step": "plan", "role": "planner" }] }),
            |_| Some(StepUse::Agent),
        )
        .expect("valid flow"),
    );
    assert_eq!(none, asking(0));

    let gated = Script::default();
    let (walked, held) = walk(&gated, &plan(true), none, 10).await;
    assert_eq!(
        walked,
        Ok(Round::Routed(
            Outcome::try_from("approve".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    assert_eq!(
        gated.asked(),
        [
            "start 1",
            "run 1",
            "end 1 done",
            "release 1",
            "ask 1 Approve?"
        ]
    );
    let request = gated.requests.borrow()[0].clone();
    assert!(!request.may_ask);
    assert_eq!(
        serde_json::to_value(&request).expect("serializes"),
        json!({
            "key": { "run_id": request.key.run_id, "seq": 1 },
            "entry": "plan",
            "step": "plan",
            "role": "planner",
            "visit": 1
        })
    );

    let failing = Script {
        ends: [(1, "fail")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&failing, &plan(true), none, 10).await;
    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));
    assert_eq!(
        failing.asked(),
        ["start 1", "run 1", "end 1 failed", "stop 1"]
    );

    let asking_anyway = Script {
        ends: [(1, "question")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&asking_anyway, &plan(false), none, 10).await;
    assert_eq!(walked, Ok(Round::Routed(Outcome::question())));
    assert_eq!(
        asking_anyway.asked(),
        ["start 1", "run 1", "end 1 question", "release 1"]
    );
}

/// A Flow that lets agents ask none (`FACTORY_QUESTIONS=0`) offers no
/// question, as one that sets none.
#[test]
fn zero_questions_offers_none() {
    let mut flow = Flow::parse(
        json!({ "entries": [{ "id": "plan", "step": "plan", "role": "planner" }] }),
        |_| Some(StepUse::Agent),
    )
    .expect("valid flow");
    flow.allow_questions(0);
    let questions = Questions::of(&flow);
    assert!(!questions.may_ask);
    let key = StepKey {
        run_id: RunId::generate(),
        seq: 1,
    };
    let request = questions.offer(StepRequest::of(key, &flow.entries[0], 1), 0);
    assert!(!request.may_ask);
    assert_eq!(request.questions_left, 0);
}

/// Only a Step offered questions asks one: its `question` hand back, cut
/// to fit a Gate, opens a question Gate whose one answer is `answer`.
#[test]
fn only_a_step_offered_questions_asks() {
    let key = StepKey {
        run_id: RunId::generate(),
        seq: 1,
    };
    let entry = plan(false);
    let asked = StepReport {
        outcome: Outcome::question(),
        output: "  CSV or JSON?\n".to_string(),
    };
    let done = StepReport {
        outcome: Outcome::done(),
        output: "The plan".to_string(),
    };
    let offered = asking(2).offer(StepRequest::of(key, &entry, 1), 0);
    let not_offered = StepRequest::of(key, &entry, 1);

    let question = asking(2).asked(&offered, &asked).expect("a question");
    assert_eq!(asking(2).asked(&offered, &done), None);
    assert_eq!(asking(2).asked(&not_offered, &asked), None);
    assert_eq!(asking(0).asked(&offered, &asked), None);
    let opening = Question::opening(question, key);
    assert_eq!(opening.question, "CSV or JSON?");
    assert_eq!(opening.answers, [Outcome::answer()]);
}

/// An agent's question keeps the asking Step's session (no release), goes
/// to the owner, and the same Role goes on at the next place, as the same
/// visit, from the asking Step, with one question fewer left; once it ends
/// it lets go of the session, and nothing stays held.
#[tokio::test]
async fn a_question_keeps_the_session_and_the_role_goes_on_with_the_answer() {
    let script = Script {
        ends: [(1, "question")].into(),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(true), asking(2), 10).await;

    assert_eq!(
        walked,
        Ok(Round::Routed(
            Outcome::try_from("approve".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    assert_eq!(
        script.asked(),
        [
            "start 1",
            "run 1",
            "end 1 question",
            "ask 1 Question of step 1?",
            "start 2",
            "run 2",
            "end 2 done",
            "release 2",
            "ask 2 Approve?"
        ]
    );
    let requests = script.requests.borrow();
    assert_eq!((requests[0].resumes, requests[0].questions_left), (None, 2));
    let going_on = &requests[1];
    assert_eq!(going_on.resumes, Some(1));
    assert_eq!(going_on.questions_left, 1);
    assert_eq!(
        (going_on.entry.clone(), going_on.visit, going_on.may_ask),
        (
            EntryId::try_from("plan".to_string()).expect("valid"),
            1,
            true
        )
    );
}

/// One question past the Step's ends the Run for a person, with every Step
/// of the walk still held, for the workflow to stop their session.
#[tokio::test]
async fn a_question_past_the_step_s_ends_the_run_for_a_person() {
    let script = Script {
        ends: [(1, "question"), (2, "question"), (3, "question")].into(),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(2), 10).await;

    assert_eq!(walked, Ok(Round::Ended(RunOutcome::Manual)));
    assert_eq!(held, [1, 2, 3]);
    let asked = script.asked();
    assert!(!asked.iter().any(|asked| asked.starts_with("release")));
    assert!(!asked.contains(&"ask 3 Question of step 3?".to_string()));
    let left: Vec<u8> = script
        .requests
        .borrow()
        .iter()
        .map(|request| request.questions_left)
        .collect();
    assert_eq!(left, [2, 1, 0]);
}

/// Work going on from a question that fails for good, as before it
/// claimed the asking Step's session, has the asking Step's agent stopped
/// too.
#[tokio::test]
async fn work_going_on_that_fails_stops_the_asking_step_too() {
    let script = Script {
        ends: [(1, "question"), (2, "fail")].into(),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(2), 10).await;

    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));
    assert!(held.is_empty());
    assert_eq!(
        &script.asked()[4..],
        ["start 2", "run 2", "end 2 failed", "stop 2", "stop 1"]
    );
}

/// A Run cancelled while the Step going on from a question works, as
/// before it claimed the asking Step's session, holds both, so the
/// workflow stops the asking Step's session as well.
#[tokio::test]
async fn a_run_cancelled_while_going_on_holds_the_asking_step() {
    let script = Script {
        ends: [(1, "question"), (2, "fail")].into(),
        cancelled: true,
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(2), 10).await;

    assert_eq!(walked, Ok(Round::Ended(RunOutcome::Cancelled)));
    assert_eq!(held, [1, 2]);
}

/// A question Gate that cannot be asked fails the walk with the asking
/// Step's agent stopped first, rather than left holding its session.
#[tokio::test]
async fn a_gate_that_cannot_be_asked_stops_the_held_steps() {
    let script = Script {
        ends: [(1, "question")].into(),
        ask_fails: true,
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(2), 10).await;

    assert_eq!(walked, Err(Failed));
    assert!(held.is_empty());
    assert_eq!(script.asked().last().map(String::as_str), Some("stop 1"));
}

/// A Run that took its most Steps when its owner answers ends `blocked`,
/// the asking Step still held, for the workflow to stop.
#[tokio::test]
async fn a_run_out_of_steps_at_an_answer_ends_blocked() {
    let script = Script {
        ends: [(1, "question")].into(),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(2), 1).await;

    assert_eq!(walked, Ok(Round::Ended(RunOutcome::Blocked)));
    assert_eq!(held, [1]);
}

/// Every Standing Rule on.
const EVERY_RULE: OrchestratorDefinition = OrchestratorDefinition {
    plan: true,
    blockers: true,
    merge: true,
};

/// A `plan` entry whose Gate is of the `plan` kind, as the built-in Flows
/// name it.
fn plan_of_kind() -> Entry {
    let entry = json!({
        "id": "plan",
        "step": "plan",
        "role": "planner",
        "gate": { "kind": "plan", "question": "Approve?", "answers": ["approve", "revise"] }
    });
    let flow =
        Flow::parse(json!({ "entries": [entry] }), |_| Some(StepUse::Agent)).expect("valid flow");
    flow.entries[0].clone()
}

/// With every Standing Rule on, an agent's question still waits for the
/// Run's owner: a question Gate is never put to the rules. The plan Gate
/// of the Step that goes on with the answer is the Flow's, and its rule
/// answers it.
#[tokio::test]
async fn a_question_waits_for_a_person_with_every_rule_on() {
    let script = Script {
        ends: [(1, "question")].into(),
        rules: Some(EVERY_RULE),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan_of_kind(), asking(2), 10).await;

    assert_eq!(
        walked,
        Ok(Round::Routed(
            Outcome::try_from("approve".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    assert_eq!(
        script.asked(),
        [
            "start 1",
            "run 1",
            "end 1 question",
            "ask 1 Question of step 1?",
            "start 2",
            "run 2",
            "end 2 done",
            "release 2",
            "rule 2 approve"
        ]
    );
}

/// A Gate the Flow names no kind of waits for a person whatever rules
/// the Run has.
#[tokio::test]
async fn a_gate_of_no_kind_waits_for_a_person_with_every_rule_on() {
    let script = Script {
        rules: Some(EVERY_RULE),
        ..Script::default()
    };

    walk(&script, &plan(true), asking(0), 10)
        .await
        .0
        .expect("walked");

    assert_eq!(
        script.asked(),
        [
            "start 1",
            "run 1",
            "end 1 done",
            "release 1",
            "ask 1 Approve?"
        ]
    );
}
