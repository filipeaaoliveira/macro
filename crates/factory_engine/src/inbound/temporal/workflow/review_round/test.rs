//! A review round's walk over a stand-in for Temporal, which answers each
//! activity as the test scripts it and keeps what it was asked.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::num::NonZeroU8;

use factory_records::domain::models::{EntryId, Outcome, Role, RunId, RunOutcome};
use factory_records::domain::standing_rules::{OrchestratorDefinition, RuleGateKind};

use super::{Panel, Round, review_round};
use crate::domain::flow::{Entry, GateDefinition, StepName};
use crate::domain::gate::{GateOpening, GateTelling};
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::domain::standing_rules::{GateFacts, rule_answer};
use crate::inbound::temporal::workflow::work::{Work, WorkFailure};
use crate::inbound::temporal::workflow::{Asking, Questions};

/// An activity that failed.
#[derive(Debug, PartialEq, Eq)]
struct Failed;

/// What the stand-in answers, and what it was asked, in order.
#[derive(Default)]
struct Script {
    opened: Option<Result<bool, Failed>>,
    starts: VecDeque<StepStart>,
    /// What each reviewer's work comes to, by its place in the entry.
    worked: Vec<Result<(), WorkFailure>>,
    cancelled: bool,
    /// What the Step at a place ends with, when not `pass`: `question` for
    /// a reviewer that asks the Run's owner, `fail` for work that fails for
    /// good, or `no_result` for a reviewer that ends without a review.
    ends: BTreeMap<u16, &'static str>,
    closed: Option<Outcome>,
    /// The Run's Standing Rules; none for a Run created before Runs kept
    /// them.
    rules: Option<OrchestratorDefinition>,
    asked: RefCell<Vec<String>>,
    closings: RefCell<Vec<ReviewRoundClosing>>,
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
        true
    }

    async fn open(&self, opening: ReviewRoundOpening) -> Result<bool, Failed> {
        self.note(format!("open {}", opening.seq));
        match &self.opened {
            Some(Ok(opened)) => Ok(*opened),
            Some(Err(_)) => Err(Failed),
            None => Ok(true),
        }
    }

    async fn start(&self, request: StepRequest) -> Result<StepStart, Failed> {
        self.note(format!("start {}", request.key.seq));
        Ok(self
            .starts
            .get(usize::from(request.key.seq - 3))
            .copied()
            .unwrap_or(StepStart::Started))
    }

    async fn run(&self, request: &StepRequest) -> Result<StepReport, WorkFailure> {
        self.run_all(std::slice::from_ref(request))
            .await
            .pop()
            .unwrap_or(Err(WorkFailure::Work))
    }

    async fn run_all(&self, requests: &[StepRequest]) -> Vec<Result<StepReport, WorkFailure>> {
        let seqs: Vec<String> = requests.iter().map(|r| r.key.seq.to_string()).collect();
        self.note(format!("run {}", seqs.join(",")));
        requests
            .iter()
            .enumerate()
            .map(|(index, request)| {
                let ends = self.ends.get(&request.key.seq).copied();
                match (self.worked.get(index), ends) {
                    (Some(Err(failure)), _) => Err(*failure),
                    (_, Some("fail")) => Err(WorkFailure::Work),
                    (_, Some("no_result")) => Ok(StepReport {
                        outcome: Outcome::no_result(),
                        output: "Stopped here.".to_string(),
                    }),
                    (_, Some("question")) => Ok(StepReport {
                        outcome: Outcome::question(),
                        output: format!("Question of step {}?", request.key.seq),
                    }),
                    _ => Ok(StepReport {
                        outcome: Outcome::try_from("pass".to_string()).expect("valid"),
                        output: "Checked it.".to_string(),
                    }),
                }
            })
            .collect()
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
                repeated_blockers: false,
            };
            if let Some(ruled) = rule_answer(self.rules, &facts, &opening.answers) {
                self.note(format!("rule {} {}", opening.key.seq, ruled.answer));
                return Ok(ruled.answer);
            }
        }
        self.note(format!("ask {} {}", opening.key.seq, opening.question));
        Ok(opening.answers[0].clone())
    }

    async fn tell(&self, telling: GateTelling) {
        self.note(format!("tell {} {}", telling.key.seq, telling.question));
    }

    async fn close(&self, closing: ReviewRoundClosing) -> Result<Outcome, Failed> {
        self.note(format!("close {}", closing.seq));
        self.closings.borrow_mut().push(closing);
        Ok(self
            .closed
            .clone()
            .unwrap_or_else(|| Outcome::try_from("pass".to_string()).expect("valid")))
    }
}

/// The review panel of three reviewers, at most `max` rounds.
fn panel_entry(max: u8) -> Entry {
    Entry {
        id: EntryId::try_from("review".to_string()).expect("entry"),
        step: StepName::try_from("review".to_string()).expect("step"),
        role: None,
        reviewers: ["correctness-reviewer", "scope-reviewer", "tests-reviewer"]
            .map(|role| Role::try_from(role.to_string()).expect("role"))
            .to_vec(),
        when: Vec::new(),
        max_visits: NonZeroU8::new(max),
        on_exhausted: Vec::new(),
        next: None,
        on: Default::default(),
        gate: None,
    }
}

/// Walk the round that starts at Step 3, its entry's `visit`, on `script`.
async fn walk(script: &Script, visit: u16) -> (Result<Round, Failed>, Vec<u16>) {
    walk_entry(script, &panel_entry(3), visit).await
}

/// [`walk`] the round of `entry`.
async fn walk_entry(
    script: &Script,
    entry: &Entry,
    visit: u16,
) -> (Result<Round, Failed>, Vec<u16>) {
    let panel = Panel {
        run_id: RunId::generate(),
        entry,
        visit,
        seq: 3,
        questions: Questions {
            may_ask: true,
            most: 2,
            owner_answers: true,
        },
    };
    let mut held = Vec::new();
    let mut next = 6;
    let mut resume = || {
        let seq = next;
        next += 1;
        Some(seq)
    };
    let round = review_round(script, &panel, &mut held, &mut resume).await;
    (round, held.iter().map(|request| request.key.seq).collect())
}

/// Every reviewer starts, works at once, ends and lets go of its session in
/// the entry's order; then the round closes, and its outcome is routed.
#[tokio::test]
async fn a_round_runs_its_reviewers_at_once_and_routes_its_outcome() {
    let script = Script::default();

    let (round, held) = walk(&script, 1).await;

    assert_eq!(
        round,
        Ok(Round::Routed(
            Outcome::try_from("pass".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    assert_eq!(
        script.asked(),
        [
            "open 3",
            "start 3",
            "start 4",
            "start 5",
            "run 3,4,5",
            "end 3 pass",
            "release 3",
            "end 4 pass",
            "release 4",
            "end 5 pass",
            "release 5",
            "close 3"
        ]
    );
    assert!(!script.closings.borrow()[0].last);
}

/// A reviewer whose work failed for good ends failed and has its agent
/// stopped; the others end and let go, and the round closes as failed.
#[tokio::test]
async fn a_failed_reviewer_is_stopped_and_fails_the_round() {
    let script = Script {
        worked: vec![Ok(()), Err(WorkFailure::Work), Ok(())],
        ..Script::default()
    };

    walk(&script, 1).await.0.expect("walked");

    let asked = script.asked();
    assert!(asked.contains(&"end 4 failed".to_string()));
    assert!(asked.contains(&"stop 4".to_string()));
    assert!(!asked.contains(&"release 4".to_string()));
    assert!(asked.contains(&"release 3".to_string()));
    assert!(script.closings.borrow()[0].failed);
}

/// A round that failed only because its reviewers' work failed for
/// infrastructure is routed as infrastructure's; one where a reviewer's
/// work failed for itself too is routed as any failed round.
#[tokio::test]
async fn a_round_failed_by_infrastructure_alone_is_routed_as_such() {
    let broke = Script {
        worked: vec![Ok(()), Err(WorkFailure::Infrastructure), Ok(())],
        closed: Some(Outcome::failed()),
        ..Script::default()
    };
    let (round, _) = walk(&broke, 1).await;
    assert_eq!(round, Ok(Round::FailedForInfrastructure));

    let mixed = Script {
        worked: vec![
            Err(WorkFailure::Work),
            Err(WorkFailure::Infrastructure),
            Ok(()),
        ],
        closed: Some(Outcome::failed()),
        ..Script::default()
    };
    let (round, _) = walk(&mixed, 1).await;
    assert_eq!(round, Ok(Round::Routed(Outcome::failed())));

    // A reviewer that ended without a review fails the round as its work's,
    // beside one whose work failed for infrastructure.
    let unreviewed = Script {
        worked: vec![Ok(()), Err(WorkFailure::Infrastructure), Ok(())],
        ends: [(5, "no_result")].into(),
        closed: Some(Outcome::failed()),
        ..Script::default()
    };
    let (round, _) = walk(&unreviewed, 1).await;
    assert_eq!(round, Ok(Round::Routed(Outcome::failed())));
}

/// A Run cancelled while its reviewers work ends cancelled, every
/// reviewer's agent still held for the workflow to stop.
#[tokio::test]
async fn a_round_cancelled_mid_work_holds_every_reviewer() {
    let script = Script {
        worked: vec![
            Err(WorkFailure::Work),
            Err(WorkFailure::Work),
            Err(WorkFailure::Work),
        ],
        cancelled: true,
        ..Script::default()
    };

    let (round, held) = walk(&script, 1).await;

    assert_eq!(round, Ok(Round::Ended(RunOutcome::Cancelled)));
    assert_eq!(held, [3, 4, 5]);
    assert!(
        !script
            .asked()
            .iter()
            .any(|asked| asked.starts_with("close"))
    );
}

/// A round that could not open runs no Step and routes `failed`; one whose
/// opening failed for good does too.
#[tokio::test]
async fn a_round_that_cannot_open_runs_no_step() {
    for opened in [Ok(false), Err(Failed)] {
        let script = Script {
            opened: Some(opened),
            ..Script::default()
        };

        let (round, held) = walk(&script, 1).await;

        assert_eq!(round, Ok(Round::Routed(Outcome::failed())));
        assert!(held.is_empty());
        assert_eq!(script.asked(), ["open 3"]);
    }
}

/// A Run that ended before the second reviewer's Step started ends as it
/// did; no reviewer works.
#[tokio::test]
async fn a_run_that_ended_mid_start_ends_the_round_as_it_did() {
    let script = Script {
        starts: [
            StepStart::Started,
            StepStart::RunEnded(RunOutcome::Cancelled),
        ]
        .into(),
        ..Script::default()
    };

    let (round, held) = walk(&script, 1).await;

    assert_eq!(round, Ok(Round::Ended(RunOutcome::Cancelled)));
    assert!(held.is_empty());
    assert_eq!(script.asked(), ["open 3", "start 3", "start 4"]);
}

/// The last round its entry runs says so when it closes, so Blockers it
/// still finds end the Run for a person rather than go to a fix.
#[tokio::test]
async fn the_last_round_says_so_when_it_closes() {
    let script = Script::default();

    walk(&script, 3).await.0.expect("walked");

    assert!(script.closings.borrow()[0].last);
}

/// A reviewer that asks the Run's owner a question ends its Step and keeps
/// its session; the others end and let go as usual. Once every reviewer
/// ended, the question goes to the owner, the reviewer goes on with the
/// answer at the next place, in the asking Step's session, and the round
/// closes on every reviewer's result, not as failed.
#[tokio::test]
async fn a_reviewer_s_question_waits_for_the_answer_and_the_round_goes_on() {
    let script = Script {
        ends: [(4, "question")].into(),
        ..Script::default()
    };

    let (round, held) = walk(&script, 1).await;

    assert_eq!(
        round,
        Ok(Round::Routed(
            Outcome::try_from("pass".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    assert_eq!(
        script.asked(),
        [
            "open 3",
            "start 3",
            "start 4",
            "start 5",
            "run 3,4,5",
            "end 3 pass",
            "release 3",
            "end 4 question",
            "end 5 pass",
            "release 5",
            "ask 4 Question of step 4?",
            "start 6",
            "run 6",
            "end 6 pass",
            "release 6",
            "close 3"
        ]
    );
    assert!(!script.closings.borrow()[0].failed);
}

/// A reviewer that asks again goes on again, each time at the next place;
/// one question past its Step's questions closes the round as failed and
/// ends the Run for a person, its session still held, for the workflow to
/// stop.
#[tokio::test]
async fn a_reviewer_asking_past_its_questions_ends_the_run_for_a_person() {
    let script = Script {
        ends: [(3, "question"), (6, "question"), (7, "question")].into(),
        ..Script::default()
    };

    let (round, held) = walk(&script, 1).await;

    assert_eq!(round, Ok(Round::Ended(RunOutcome::Manual)));
    assert_eq!(held, [3, 6, 7]);
    let asked = script.asked();
    let after_work: Vec<&str> = asked
        .iter()
        .skip_while(|asked| !asked.starts_with("ask"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        after_work,
        [
            "ask 3 Question of step 3?",
            "start 6",
            "run 6",
            "end 6 question",
            "ask 6 Question of step 6?",
            "start 7",
            "run 7",
            "end 7 question",
            "close 3"
        ]
    );
    assert!(script.closings.borrow()[0].failed);
}

/// A Run whose reviewers may not ask (its Flow predates questions) never
/// takes the question's path: a Step that somehow ends `question` reads as
/// any other review and lets go of its session.
#[tokio::test]
async fn reviewers_of_a_flow_without_questions_never_ask() {
    let script = Script {
        ends: [(4, "question")].into(),
        ..Script::default()
    };
    let entry = panel_entry(3);
    let panel = Panel {
        run_id: RunId::generate(),
        entry: &entry,
        visit: 1,
        seq: 3,
        questions: Questions {
            may_ask: false,
            most: 0,
            owner_answers: true,
        },
    };
    let mut held = Vec::new();

    review_round(&script, &panel, &mut held, &mut || Some(6))
        .await
        .expect("walked");

    let asked = script.asked();
    assert!(asked.contains(&"release 4".to_string()));
    assert!(!asked.iter().any(|asked| asked.starts_with("ask")));
}

/// A reviewer that goes on from its question and fails, before or after
/// it claimed the asking Step's session, has both its own agent and the
/// asking Step's stopped, and the round fails.
#[tokio::test]
async fn a_failed_reviewer_going_on_stops_the_asking_step_too() {
    let script = Script {
        ends: [(3, "question"), (6, "fail")].into(),
        ..Script::default()
    };

    let (round, held) = walk(&script, 1).await;

    assert_eq!(
        round,
        Ok(Round::Routed(
            Outcome::try_from("pass".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    let asked = script.asked();
    let after_work: Vec<&str> = asked
        .iter()
        .skip_while(|asked| !asked.starts_with("ask"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        after_work,
        [
            "ask 3 Question of step 3?",
            "start 6",
            "run 6",
            "end 6 failed",
            "stop 6",
            "stop 3",
            "close 3"
        ]
    );
    assert!(script.closings.borrow()[0].failed);
}

/// A Run cancelled while a reviewer goes on from its question holds the
/// asking Step as well as the one going on, so both are stopped with the
/// Run, whether or not the session was claimed yet.
#[tokio::test]
async fn a_round_cancelled_while_a_reviewer_goes_on_holds_the_asking_step() {
    let script = Script {
        ends: [(3, "question"), (6, "fail")].into(),
        cancelled: true,
        ..Script::default()
    };

    let (round, held) = walk(&script, 1).await;

    assert_eq!(round, Ok(Round::Ended(RunOutcome::Cancelled)));
    assert_eq!(held, [3, 6]);
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
    queue_limit: None,
    token_budget: None,
    orchestrator: factory_records::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
};

/// The review panel of three reviewers, at most three rounds, with the
/// `code` Flow's Blockers Gate after a round ends `changes`.
fn gated_panel() -> Entry {
    Entry {
        gate: Some(GateDefinition {
            question: "Blockers?".to_string(),
            answers: ["fix", "stop"]
                .map(|answer| Outcome::try_from(answer.to_string()).expect("valid"))
                .to_vec(),
            after: Some(Outcome::try_from("changes".to_string()).expect("valid")),
            kind: Some(RuleGateKind::Blockers),
            person_only: false,
            notify_only: false,
        }),
        ..panel_entry(3)
    }
}

/// A round whose reviewer asks the Run's owner, with every Standing Rule
/// on: the question waits for a person, the round closes only once the
/// reviewer went on with the answer, and only then the Blockers Gate
/// opens, on the round's latest Step, and its rule sends them to a fix.
#[tokio::test]
async fn a_blockers_gate_after_a_reviewer_s_question_gets_its_rule_answer() {
    let script = Script {
        ends: [(4, "question")].into(),
        closed: Some(Outcome::try_from("changes".to_string()).expect("valid")),
        rules: Some(EVERY_RULE),
        ..Script::default()
    };

    let (round, held) = walk_entry(&script, &gated_panel(), 1).await;

    assert_eq!(
        round,
        Ok(Round::Routed(
            Outcome::try_from("fix".to_string()).expect("valid")
        ))
    );
    assert!(held.is_empty());
    assert_eq!(
        script.asked(),
        [
            "open 3",
            "start 3",
            "start 4",
            "start 5",
            "run 3,4,5",
            "end 3 pass",
            "release 3",
            "end 4 question",
            "end 5 pass",
            "release 5",
            "ask 4 Question of step 4?",
            "start 6",
            "run 6",
            "end 6 pass",
            "release 6",
            "close 3",
            "rule 6 fix"
        ]
    );
}

/// Without rules, the Blockers Gate of a round with no question waits for
/// a person on the round's last reviewer's Step once the round closes, and
/// a round that ends otherwise asks nothing.
#[tokio::test]
async fn a_blockers_gate_waits_for_a_person_once_the_round_closes() {
    let script = Script {
        closed: Some(Outcome::try_from("changes".to_string()).expect("valid")),
        ..Script::default()
    };

    let (round, _) = walk_entry(&script, &gated_panel(), 1).await;

    assert_eq!(
        round,
        Ok(Round::Routed(
            Outcome::try_from("fix".to_string()).expect("valid")
        ))
    );
    assert_eq!(
        script.asked().last().map(String::as_str),
        Some("ask 5 Blockers?")
    );
    assert_eq!(script.asked()[script.asked().len() - 2], "close 3");

    let passed = Script {
        rules: Some(EVERY_RULE),
        ..Script::default()
    };
    let (round, _) = walk_entry(&passed, &gated_panel(), 1).await;
    assert_eq!(
        round,
        Ok(Round::Routed(
            Outcome::try_from("pass".to_string()).expect("valid")
        ))
    );
    assert_eq!(passed.asked().last().map(String::as_str), Some("close 3"));
}

/// The Blockers Gate's rule sends no more fixes than the panel's rounds
/// leave room for: at the third visit of a three-round panel's Gate (two
/// fixes sent), it waits for a person.
#[tokio::test]
async fn a_blockers_gate_past_the_rule_s_fixes_waits_for_a_person() {
    let script = Script {
        closed: Some(Outcome::try_from("changes".to_string()).expect("valid")),
        rules: Some(EVERY_RULE),
        ..Script::default()
    };

    walk_entry(&script, &gated_panel(), 3)
        .await
        .0
        .expect("walked");

    assert_eq!(
        script.asked().last().map(String::as_str),
        Some("ask 5 Blockers?")
    );
}

/// A notify-only Gate on a review panel tells the Run's owner, on the
/// round's last Step, once the round ends with the outcome it names, and
/// the Flow routes that outcome without asking anyone.
#[tokio::test]
async fn a_notify_only_gate_on_a_panel_tells_and_routes_the_round_s_outcome() {
    let told_panel = Entry {
        gate: Some(GateDefinition {
            question: "The review found Blockers.".to_string(),
            answers: Vec::new(),
            after: Some(Outcome::try_from("changes".to_string()).expect("valid")),
            kind: None,
            person_only: false,
            notify_only: true,
        }),
        ..panel_entry(3)
    };
    let script = Script {
        closed: Some(Outcome::try_from("changes".to_string()).expect("valid")),
        ..Script::default()
    };

    let (round, _) = walk_entry(&script, &told_panel, 1).await;

    assert_eq!(
        round,
        Ok(Round::Routed(
            Outcome::try_from("changes".to_string()).expect("valid")
        ))
    );
    let asked = script.asked();
    assert_eq!(
        &asked[asked.len() - 2..],
        ["close 3", "tell 5 The review found Blockers."]
    );
}
