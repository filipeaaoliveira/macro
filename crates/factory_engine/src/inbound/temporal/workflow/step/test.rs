//! A Step's walk over a stand-in for Temporal, which answers each activity
//! as the test scripts it and keeps what it was asked.

use std::cell::RefCell;
use std::collections::BTreeMap;

use factory_records::domain::models::{EntryId, Outcome, RunId, RunOutcome, StepKey};
use factory_records::domain::standing_rules::OrchestratorDefinition;
use serde_json::json;

use super::walk_step;
use crate::domain::flow::{Entry, Flow, StepUse};
use crate::domain::gate::{GateOpening, GateTelling};
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::domain::standing_rules::{GateFacts, rule_answer};
use crate::inbound::temporal::workflow::review_round::Round;
use crate::inbound::temporal::workflow::work::{Work, WorkFailure};
use crate::inbound::temporal::workflow::{Asking, Question, Questions};

/// An activity that failed.
#[derive(Debug, PartialEq, Eq)]
struct Failed;

/// What the stand-in answers, and what it was asked, in order.
#[derive(Default)]
struct Script {
    /// What the Step at a place ends with, when not `done`: `question`,
    /// `fail` for work that fails for good, `broke` for work that fails
    /// for good for infrastructure, or `refused` for a session its agent's
    /// machine refused.
    ends: BTreeMap<u16, &'static str>,
    cancelled: bool,
    /// Asking a Gate fails, other than by the Run's cancellation.
    ask_fails: bool,
    /// The Run.s history passed a Step that ended `no_result` before the
    /// workflow asked its owner about one.
    before_no_result_gates: bool,
    /// The answers a person gives the Gates the walk asks, in order; the
    /// first a Gate offers once these run out.
    answers: RefCell<Vec<Outcome>>,
    /// What each Gate the walk asked says of its Standing Rule, in order.
    asked_with: RefCell<Vec<Option<Asking>>>,
    /// The Run's Standing Rules; none for a Run created before Runs kept
    /// them.
    rules: Option<OrchestratorDefinition>,
    asked: RefCell<Vec<String>>,
    /// The output each Step ended with, in order.
    outputs: RefCell<Vec<String>>,
    requests: RefCell<Vec<StepRequest>>,
    /// The Run used its token budget: every check before an agent Step
    /// stops it. A Run within it is not noted, as one without a budget.
    budget_spent: bool,
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

    fn asks_on_no_result(&self) -> bool {
        !self.before_no_result_gates
    }

    async fn within_budget(&self, _run_id: RunId) -> bool {
        if self.budget_spent {
            self.note("budget spent".to_string());
        }
        !self.budget_spent
    }

    async fn open(&self, _opening: ReviewRoundOpening) -> Result<bool, Failed> {
        unreachable!("a Step's walk opens no round")
    }

    async fn start(&self, request: StepRequest) -> Result<StepStart, Failed> {
        self.note(format!("start {}", request.key.seq));
        self.requests.borrow_mut().push(request);
        Ok(StepStart::Started)
    }

    async fn run(&self, request: &StepRequest) -> Result<StepReport, WorkFailure> {
        self.note(format!("run {}", request.key.seq));
        match self.ends.get(&request.key.seq).copied() {
            Some("fail") => Err(WorkFailure::Work),
            Some("broke") => Err(WorkFailure::Infrastructure),
            Some("refused") => Err(WorkFailure::Refused(REFUSAL.to_string())),
            Some("question") => Ok(StepReport {
                outcome: Outcome::question(),
                output: format!("Question of step {}?", request.key.seq),
            }),
            Some("no_result") => Ok(StepReport {
                outcome: Outcome::no_result(),
                output: format!("Step {} stopped here.", request.key.seq),
            }),
            Some("blockers") => Ok(StepReport {
                outcome: Outcome::try_from("blockers".to_string()).expect("valid"),
                output: "A clause does not hold".to_string(),
            }),
            _ => Ok(StepReport {
                outcome: Outcome::done(),
                output: "The plan".to_string(),
            }),
        }
    }

    async fn run_all(&self, _requests: &[StepRequest]) -> Vec<Result<StepReport, WorkFailure>> {
        unreachable!("a Step's walk runs one Step at a time")
    }

    async fn end(&self, ending: StepEnding) -> Result<(), Failed> {
        self.note(format!("end {} {}", ending.key.seq, ending.report.outcome));
        self.outputs.borrow_mut().push(ending.report.output);
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
        self.asked_with.borrow_mut().push(rule);
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
                repeated_blockers: false,
                plan_blockers_left: false,
                dismissed: false,
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
        let mut answers = self.answers.borrow_mut();
        if answers.is_empty() {
            return Ok(opening.answers[0].clone());
        }
        Ok(answers.remove(0))
    }

    async fn tell(&self, telling: GateTelling) {
        self.note(format!("tell {} {}", telling.key.seq, telling.question));
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
        owner_answers: true,
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

/// A Run that used its token budget while its agent asked ends `blocked`
/// before the Role goes on with the answer, the asking Step still held for
/// the workflow to stop its session.
#[tokio::test]
async fn a_run_at_its_budget_ends_before_going_on_from_a_question() {
    let script = Script {
        ends: [(1, "question")].into(),
        budget_spent: true,
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(2), 10).await;

    assert_eq!(walked, Ok(Round::Ended(RunOutcome::Blocked)));
    assert_eq!(held, [1]);
    assert_eq!(
        script.asked(),
        [
            "start 1",
            "run 1",
            "end 1 question",
            "ask 1 Question of step 1?",
            "budget spent"
        ]
    );
}

/// A Run that used its token budget ends `blocked` on its owner's `retry`
/// of an agent that stopped without a result, before the Role goes on.
#[tokio::test]
async fn a_run_at_its_budget_ends_before_a_retry() {
    let script = Script {
        ends: [(1, "no_result")].into(),
        answers: RefCell::new(vec![Outcome::retry()]),
        budget_spent: true,
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(0), 10).await;

    assert_eq!(walked, Ok(Round::Ended(RunOutcome::Blocked)));
    assert_eq!(held, [1]);
    assert_eq!(
        script.asked().last().map(String::as_str),
        Some("budget spent")
    );
    assert!(!script.asked().iter().any(|asked| asked == "start 2"));
}

/// Every Standing Rule on.
const EVERY_RULE: OrchestratorDefinition = OrchestratorDefinition {
    stand_in: factory_records::domain::standing_rules::StandInKinds {
        plan: false,
        blockers: false,
    },
    plan: true,
    blockers: true,
    merge: true,
    owner_gates: false,
    quiet_gates: false,
    task_thread: false,
    rerun_flaky_ci: false,
    queue_limit: None,
    token_budget: None,
    orchestrator: factory_records::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
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

/// A notify-only Gate tells the Run's owner once its Step ends `done`, and
/// the Run goes on with `done`, asking no one; a Step that ends otherwise
/// tells no one.
#[tokio::test]
async fn a_notify_only_gate_tells_and_routes_the_step_s_outcome() {
    let entry = Flow::parse(
        json!({ "entries": [{
            "id": "plan",
            "step": "plan",
            "role": "planner",
            "gate": { "question": "The plan is ready.", "notify_only": true }
        }] }),
        |_| Some(StepUse::Agent),
    )
    .expect("valid flow")
    .entries[0]
        .clone();

    let told = Script::default();
    let (walked, held) = walk(&told, &entry, asking(0), 10).await;
    assert_eq!(walked, Ok(Round::Routed(Outcome::done())));
    assert!(held.is_empty());
    assert_eq!(
        told.asked(),
        [
            "start 1",
            "run 1",
            "end 1 done",
            "release 1",
            "tell 1 The plan is ready."
        ]
    );

    let failing = Script {
        ends: [(1, "fail")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&failing, &entry, asking(0), 10).await;
    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));
    assert!(
        !failing
            .asked()
            .iter()
            .any(|asked| asked.starts_with("tell")),
        "{:?}",
        failing.asked()
    );
}

/// What the walk asked, with each Gate's question cut to its first line.
fn asked_briefly(script: &Script) -> Vec<String> {
    script
        .asked()
        .into_iter()
        .map(|asked| asked.lines().next().unwrap_or_default().to_string())
        .collect()
}

/// An agent that stopped without a result keeps its session (no release)
/// and goes to the Run's owner on a Gate that shows its last message and
/// offers `retry` or `stop`. On `retry` the same Role goes on at the next
/// place, as the same visit, in that session; once it ends it lets go,
/// and the entry's Gate asks as usual.
#[tokio::test]
async fn an_agent_that_stopped_without_a_result_waits_for_the_owner_who_retries() {
    let script = Script {
        ends: [(1, "no_result")].into(),
        answers: RefCell::new(vec![Outcome::retry()]),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(true), asking(0), 10).await;

    assert_eq!(
        walked,
        Ok(Round::Routed(
            Outcome::try_from("approve".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    assert_eq!(
        asked_briefly(&script),
        [
            "start 1",
            "run 1",
            "end 1 no_result",
            "ask 1 The agent stopped without handing back a result, even once reminded. Retry the step in the same session, or stop it? The agent's last message:",
            "start 2",
            "run 2",
            "end 2 done",
            "release 2",
            "ask 2 Approve?"
        ]
    );
    assert!(script.asked()[3].ends_with("Step 1 stopped here."));
    let requests = script.requests.borrow();
    let going_on = &requests[1];
    assert_eq!(going_on.resumes, Some(1));
    assert_eq!((going_on.entry.as_str(), going_on.visit), ("plan", 1));
}

/// On `stop` the stopped Step's agent is stopped, and the Step goes on as
/// one that failed: the Flow routes `failed`, and nothing stays held.
#[tokio::test]
async fn an_owner_who_stops_a_step_without_a_result_fails_it() {
    let script = Script {
        ends: [(1, "no_result")].into(),
        answers: RefCell::new(vec![Outcome::stop()]),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(true), asking(0), 10).await;

    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));
    assert!(held.is_empty());
    assert_eq!(&asked_briefly(&script)[4..], ["stop 1"]);
}

/// A Run whose history passed a Step that ended `no_result` before the
/// workflow asked its owner replays it as the Step failed then: its agent
/// stopped, no Gate, `failed` routed.
#[tokio::test]
async fn a_step_without_a_result_before_the_marker_fails_as_before() {
    let script = Script {
        ends: [(1, "no_result")].into(),
        before_no_result_gates: true,
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(true), asking(0), 10).await;

    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));
    assert!(held.is_empty());
    assert_eq!(
        script.asked(),
        ["start 1", "run 1", "end 1 failed", "stop 1"]
    );
}

/// A Run cancelled while its owner is asked about a Step without a result
/// ends cancelled, holding the Step for the workflow to stop its session.
#[tokio::test]
async fn a_run_cancelled_at_a_step_without_a_result_holds_its_session() {
    let script = Script {
        ends: [(1, "no_result")].into(),
        ask_fails: true,
        cancelled: true,
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), asking(0), 10).await;

    assert_eq!(walked, Ok(Round::Ended(RunOutcome::Cancelled)));
    assert_eq!(held, [1]);
}

/// The Gate of a Step without a result is no Flow's Gate: no kind for a
/// Standing Rule or the stand-in to read, and kept for a person.
#[tokio::test]
async fn the_gate_of_a_step_without_a_result_has_no_kind() {
    let script = Script {
        ends: [(1, "no_result")].into(),
        answers: RefCell::new(vec![Outcome::stop()]),
        ..Script::default()
    };

    walk(&script, &plan(false), asking(0), 10)
        .await
        .0
        .expect("walked");

    assert_eq!(
        *script.asked_with.borrow(),
        [Some(Asking {
            kind: None,
            person_only: true,
            fix_rounds: 0,
            fix_limit: 0,
        })]
    );
}

/// An entry whose Gate opens after `failed`: once its Step's owner
/// answered `stop` to the Gate of an agent that stopped without a result,
/// the Flow routes `failed` without asking the entry's Gate, since the
/// Step already had its one Gate.
#[tokio::test]
async fn stopping_a_step_without_a_result_never_asks_the_entry_s_gate_after_failed() {
    let flow = Flow::parse(
        json!({ "entries": [{
            "id": "plan", "step": "plan", "role": "planner",
            "gate": { "question": "Try again?", "answers": ["again", "give_up"], "after": "failed" }
        }] }),
        |_| Some(StepUse::Agent),
    )
    .expect("valid flow");
    let script = Script {
        ends: [(1, "no_result")].into(),
        answers: RefCell::new(vec![Outcome::stop()]),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &flow.entries[0], asking(0), 10).await;

    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));
    assert!(held.is_empty());
    let asked = asked_briefly(&script);
    assert!(
        !asked.iter().any(|asked| asked.contains("Try again?")),
        "{asked:?}"
    );
    assert_eq!(asked.last().map(String::as_str), Some("stop 1"));
}

/// An `accept` entry, as the `code` Flow has it: a single Step whose
/// Blockers Gate opens on `blockers`.
fn accept() -> Entry {
    let flow = Flow::parse(
        json!({ "entries": [{
            "id": "accept",
            "step": "accept",
            "role": "verifier",
            "max_visits": 3,
            "gate": {
                "kind": "blockers",
                "after": "blockers",
                "question": "Fix it?",
                "answers": ["fix", "stop"]
            }
        }] }),
        |_| Some(StepUse::Agent),
    )
    .expect("valid flow");
    flow.entries[0].clone()
}

/// A verifier that stopped without a result opens only its owner's Gate,
/// never the entry's Blockers Gate as well. The Step that goes on from the
/// owner's `retry` is the same visit: when it finds blockers, the Blockers
/// Gate asks with the fixes of the visits before this one (none at the
/// first), which the retried Step does not add to.
#[tokio::test]
async fn a_retried_verifier_s_blockers_count_the_visits_not_the_retry() {
    let script = Script {
        ends: [(1, "no_result"), (2, "blockers")].into(),
        answers: RefCell::new(vec![Outcome::retry(), Outcome::stop()]),
        ..Script::default()
    };
    let entry = accept();
    let key = StepKey {
        run_id: RunId::generate(),
        seq: 1,
    };
    let request = asking(0).offer(StepRequest::of(key, &entry, 1), 0);
    let mut held = Vec::new();
    let mut next = 2;
    let mut resume = || {
        let seq = Some(next);
        next += 1;
        seq
    };

    let walked = walk_step(&script, &entry, request, asking(0), &mut held, &mut resume).await;

    assert_eq!(walked, Ok(Round::Routed(Outcome::stop())));
    assert_eq!(
        asked_briefly(&script),
        [
            "start 1",
            "run 1",
            "end 1 no_result",
            "ask 1 The agent stopped without handing back a result, even once reminded. Retry the step in the same session, or stop it? The agent's last message:",
            "start 2",
            "run 2",
            "end 2 blockers",
            "release 2",
            "ask 2 Fix it?"
        ]
    );
    assert_eq!(script.requests.borrow()[1].visit, 1);
    assert_eq!(
        *script.asked_with.borrow(),
        [
            Some(Asking {
                kind: None,
                person_only: true,
                fix_rounds: 0,
                fix_limit: 0,
            }),
            Some(Asking {
                kind: Some(factory_records::domain::standing_rules::RuleGateKind::Blockers),
                person_only: false,
                fix_rounds: 0,
                fix_limit: crate::domain::standing_rules::rule_fix_limit(entry.max_visits),
            }),
        ]
    );
}

/// Work that failed for good for a machine, quota or network reason is
/// routed `failed` as any failed work is, marked as infrastructure's; work
/// that failed for itself is not. A Gate that opens on `failed` takes a
/// person's answer, which is routed as theirs.
#[tokio::test]
async fn work_that_failed_for_infrastructure_is_routed_as_such() {
    let broke = Script {
        ends: [(1, "broke")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&broke, &plan(true), asking(0), 10).await;
    assert_eq!(walked, Ok(Round::FailedForInfrastructure));
    assert_eq!(
        broke.asked(),
        ["start 1", "run 1", "end 1 failed", "stop 1"]
    );

    let failing = Script {
        ends: [(1, "fail")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&failing, &plan(false), asking(0), 10).await;
    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));

    let flow = Flow::parse(
        json!({ "entries": [{
            "id": "plan", "step": "plan", "role": "planner",
            "gate": { "question": "Try again?", "answers": ["again", "give_up"], "after": "failed" }
        }] }),
        |_| Some(StepUse::Agent),
    )
    .expect("valid flow");
    let answered = Script {
        ends: [(1, "broke")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&answered, &flow.entries[0], asking(0), 10).await;
    assert_eq!(
        walked,
        Ok(Round::Routed(
            Outcome::try_from("again".to_string()).expect("valid")
        ))
    );
}

/// What the Step whose session its machine refused shows its owner.
const REFUSAL: &str =
    "The agent's machine refused the step's session: this machine does not manage repositories";

/// A Step whose session its agent's machine refused fails for
/// infrastructure, and shows its owner the machine's reason, where other
/// failed work shows a fixed text.
#[tokio::test]
async fn a_step_its_machine_refused_shows_why() {
    let refused = Script {
        ends: [(1, "refused")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&refused, &plan(true), asking(0), 10).await;
    assert_eq!(walked, Ok(Round::FailedForInfrastructure));
    assert_eq!(
        refused.asked(),
        ["start 1", "run 1", "end 1 failed", "stop 1"]
    );
    assert_eq!(*refused.outputs.borrow(), [REFUSAL]);

    let broke = Script {
        ends: [(1, "broke")].into(),
        ..Script::default()
    };
    let (walked, _) = walk(&broke, &plan(true), asking(0), 10).await;
    assert_eq!(walked, Ok(Round::FailedForInfrastructure));
    assert_eq!(
        *broke.outputs.borrow(),
        [crate::inbound::temporal::workflow::FAILED_STEP_OUTPUT]
    );
}

/// On an unattended Flow, a Benchmark trial's, an agent that stopped
/// without a result asks no one: the Step fails, its agent is stopped, and
/// the Flow routes `failed`.
#[tokio::test]
async fn an_unattended_step_without_a_result_fails_and_asks_no_one() {
    let unattended = Questions::of(
        &Flow::parse(
            json!({
                "unattended": true,
                "entries": [{ "id": "plan", "step": "plan", "role": "planner" }]
            }),
            |_| Some(StepUse::Agent),
        )
        .expect("valid flow"),
    );
    let script = Script {
        ends: [(1, "no_result")].into(),
        ..Script::default()
    };

    let (walked, held) = walk(&script, &plan(false), unattended, 10).await;

    assert_eq!(walked, Ok(Round::Routed(Outcome::failed())));
    assert!(held.is_empty());
    assert_eq!(
        script.asked(),
        ["start 1", "run 1", "end 1 failed", "stop 1"]
    );
}
