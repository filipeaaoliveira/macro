//! An entry's visit that runs one Step, walked over [`Work`], what it asks
//! of Temporal: start the Step, run its work, record its end, stop or let
//! go of its agent, and ask its entry's Gate once it ends with the outcome
//! the Gate names (`done` unless it names another). While its
//! agent asks the Run's owner questions, each goes on a question Gate, and
//! the same Role goes on with the answer in a Step of its own, at the next
//! place, in the asking Step's session.
//!
//! The asking Steps stay held until the Step that goes on from them ends:
//! it claims their session only once its work starts, so a Run cancelled,
//! or work that fails, before that still has their session stopped.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, RunOutcome, StepKey};

use super::review_round::Round;
use super::work::Work;
use super::{Asking, FAILED_STEP_OUTPUT, Questions};
use crate::domain::flow::Entry;
use crate::domain::gate::{GateOpening, GateTelling};
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::no_result::gate_question;

/// Walk the Step `request` of `entry`, and the Steps that go on from its
/// agent's questions, and return the outcome the Flow routes, or how the
/// Run ended. `held` names the Steps whose agents may still hold what
/// their work used, for the workflow to stop when the Run ends. `resume`
/// gives the place of a Step that goes on from a question, none once the
/// Run took its most Steps.
pub(super) async fn walk_step<W: Work>(
    work: &W,
    entry: &Entry,
    request: StepRequest,
    questions: Questions,
    held: &mut Vec<StepRequest>,
    resume: &mut impl FnMut() -> Option<u16>,
) -> Result<Round, W::Failure> {
    let mut request = request;
    // The Steps the current one goes on from, whose session it works in.
    let mut asking: Vec<StepRequest> = Vec::new();
    let mut asked: u8 = 0;
    loop {
        let key = request.key;
        match work.start(request.clone()).await {
            Ok(StepStart::Started) => {}
            // The Run ended before this workflow started, cancelled or
            // swept: the workflow ends as the Run did.
            Ok(StepStart::RunEnded(outcome)) => return Ok(Round::Ended(outcome)),
            Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
            Err(error) => return Err(error),
        }

        // A Step that still fails for good ends `failed`, with why as its
        // output; the Flow may route it like any other outcome. A Step
        // stopped by cancelling the Run ends the Run.
        *held = asking.clone();
        held.push(request.clone());
        let failure = || StepReport {
            outcome: Outcome::failed(),
            output: FAILED_STEP_OUTPUT.to_string(),
        };
        let (report, failed) = match work.run(&request).await {
            // An agent that stopped without a result waits for the Run's
            // owner, behind the marker: no worker before it ended a Step
            // so, so a Run's history replays as before, and a Step that
            // ends so without it fails as it did then.
            Ok(report) if report.outcome == Outcome::no_result() && !work.asks_on_no_result() => {
                (failure(), true)
            }
            Ok(report) => (report, false),
            Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
            // The owner reads the output; why the work failed is internal
            // detail, which the worker logged.
            Err(_) => (failure(), true),
        };
        let stopped = report.outcome == Outcome::no_result();
        let mut reported = report.outcome.clone();
        let question = questions.asked(&request, &report);
        let last_words = report.output.clone();
        if let Err(error) = work.end(StepEnding { key, report }).await {
            if work.cancelled() {
                return Ok(Round::Ended(RunOutcome::Cancelled));
            }
            return Err(error);
        }

        // Work that failed for good has its agent stopped once the Step's
        // end is recorded, so the Step reads as failed meanwhile, and an
        // entry that fixes or retries it never runs beside it; the Steps it
        // went on from too, as it may have failed before it claimed their
        // session. A Step that asked a question, or whose agent stopped
        // without a result, keeps its session for the Step that goes on
        // with the owner's answer. Otherwise no attempt of the Step's work
        // comes once its end is recorded, so its agent lets go of the
        // Step's session.
        if failed {
            held.clear();
            work.stop(&request).await;
            for asked in asking.iter().rev() {
                work.stop(asked).await;
            }
        } else if question.is_none() && !stopped {
            let taken = std::mem::take(held);
            if let Err(error) = work.release(request.clone()).await {
                if work.cancelled() {
                    *held = taken;
                    return Ok(Round::Ended(RunOutcome::Cancelled));
                }
                return Err(error);
            }
        }

        // An agent's question goes to the Run's owner on a question Gate;
        // the Step's Role then goes on with the answer. One past the Step's
        // questions ends the Run for a person instead, with the question in
        // the Step's output; its session is stopped with the Run.
        if let Some(question) = question {
            asked = asked.saturating_add(1);
            if asked > questions.most {
                return Ok(Round::Ended(RunOutcome::Manual));
            }
            // Only the Run's owner answers an agent: no Standing Rule is
            // asked about a question Gate.
            if ask(work, question.opening(key), None, held)
                .await?
                .is_none()
            {
                return Ok(Round::Ended(RunOutcome::Cancelled));
            }
            let Some(seq) = resume() else {
                return Ok(Round::Ended(RunOutcome::Blocked));
            };
            let next = questions.offer(
                request.going_on(StepKey {
                    run_id: key.run_id,
                    seq,
                }),
                asked,
            );
            asking.push(std::mem::replace(&mut request, next));
            continue;
        }

        // An agent that stopped without a result, even once reminded, goes
        // to the Run's owner on a Gate showing its last message: `retry`
        // has the same Role go on in a Step of its own, at the next place,
        // in the same session; `stop` stops its agent and fails the Step,
        // for the Flow to route `failed`. No Standing Rule or stand-in
        // answers it: it is no Flow's Gate, of no kind.
        if stopped {
            let opening = GateOpening {
                key,
                question: gate_question(&last_words),
                answers: vec![Outcome::retry(), Outcome::stop()],
            };
            let person = Asking {
                kind: None,
                person_only: true,
                fix_rounds: 0,
                fix_limit: 0,
            };
            let Some(answer) = ask(work, opening, Some(person), held).await? else {
                return Ok(Round::Ended(RunOutcome::Cancelled));
            };
            if answer == Outcome::retry() {
                let Some(seq) = resume() else {
                    return Ok(Round::Ended(RunOutcome::Blocked));
                };
                let next = questions.offer(
                    request.going_on(StepKey {
                        run_id: key.run_id,
                        seq,
                    }),
                    asked,
                );
                asking.push(std::mem::replace(&mut request, next));
                continue;
            }
            for stopping in std::mem::take(held).iter().rev() {
                work.stop(stopping).await;
            }
            reported = Outcome::failed();
        }

        // The entry's Gate opens on the outcome it names (`done` unless it
        // says otherwise), and its kind, from the Flow, is what the Run's
        // Standing Rules may answer. A notify-only Gate tells the Run's
        // owner and lets the Run go on with that outcome.
        let outcome = match &entry.gate {
            Some(gate) if reported == gate.opened_by() && gate.notify_only => {
                let telling = GateTelling {
                    key,
                    question: gate.question.clone(),
                };
                work.tell(telling).await;
                reported
            }
            Some(gate) if reported == gate.opened_by() => {
                let opening = GateOpening {
                    key,
                    question: gate.question.clone(),
                    answers: gate.answers.clone(),
                };
                let rule = Asking {
                    kind: gate.kind,
                    person_only: gate.person_only,
                    fix_rounds: 0,
                    fix_limit: 0,
                };
                match ask(work, opening, Some(rule), held).await? {
                    Some(answer) => answer,
                    None => return Ok(Round::Ended(RunOutcome::Cancelled)),
                }
            }
            _ => reported,
        };
        return Ok(Round::Routed(outcome));
    }
}

/// Ask the Gate `opening`, of the Flow's when `rule` says what its
/// Standing Rule reads, and return the answer: none once the Run was
/// cancelled. A Gate that cannot be asked otherwise fails the workflow, its
/// held Steps' agents stopped first.
pub(super) async fn ask<W: Work>(
    work: &W,
    opening: GateOpening,
    rule: Option<Asking>,
    held: &mut Vec<StepRequest>,
) -> Result<Option<Outcome>, W::Failure> {
    match work.ask(opening, rule).await {
        Ok(answer) => Ok(Some(answer)),
        Err(_) if work.cancelled() => Ok(None),
        Err(error) => {
            for request in std::mem::take(held).iter().rev() {
                work.stop(request).await;
            }
            Err(error)
        }
    }
}
