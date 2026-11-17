//! A review round's walk over a stand-in for Temporal, which answers each
//! activity as the test scripts it and keeps what it was asked.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::num::NonZeroU8;

use factory_records::domain::models::{EntryId, Outcome, Role, RunId, RunOutcome};

use super::{Panel, Round, RoundWork, review_round};
use crate::domain::flow::{Entry, StepName};
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};

/// An activity that failed.
#[derive(Debug, PartialEq, Eq)]
struct Failed;

/// What the stand-in answers, and what it was asked, in order.
#[derive(Default)]
struct Script {
    opened: Option<Result<bool, Failed>>,
    starts: VecDeque<StepStart>,
    /// What each reviewer's work comes to, by its place in the entry.
    worked: Vec<Result<(), Failed>>,
    cancelled: bool,
    closed: Option<Outcome>,
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

impl RoundWork for Script {
    type Failure = Failed;

    fn cancelled(&self) -> bool {
        self.cancelled
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

    async fn run_all(&self, requests: &[StepRequest]) -> Vec<Result<StepReport, Failed>> {
        let seqs: Vec<String> = requests.iter().map(|r| r.key.seq.to_string()).collect();
        self.note(format!("run {}", seqs.join(",")));
        (0..requests.len())
            .map(|index| match self.worked.get(index) {
                Some(Err(_)) => Err(Failed),
                _ => Ok(StepReport {
                    outcome: Outcome::try_from("pass".to_string()).expect("valid"),
                    output: "Checked it.".to_string(),
                }),
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
    let entry = panel_entry(3);
    let panel = Panel {
        run_id: RunId::generate(),
        entry: &entry,
        visit,
        seq: 3,
    };
    let mut held = Vec::new();
    let round = review_round(script, &panel, &mut held).await;
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
        worked: vec![Ok(()), Err(Failed), Ok(())],
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

/// A Run cancelled while its reviewers work ends cancelled, every
/// reviewer's agent still held for the workflow to stop.
#[tokio::test]
async fn a_round_cancelled_mid_work_holds_every_reviewer() {
    let script = Script {
        worked: vec![Err(Failed), Err(Failed), Err(Failed)],
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
