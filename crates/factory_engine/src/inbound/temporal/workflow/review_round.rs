//! A review panel's visit: one review round, walked over [`Work`], what it
//! asks of Temporal.
//!
//! A reviewer whose agent asks the Run's owner a question ends its Step
//! `question`, and keeps its session; once every reviewer's Step ended,
//! each question goes to the owner on a question Gate, one at a time in the
//! entry's order, and the reviewer goes on with the answer in a Step of its
//! own, in that session. The round closes once every reviewer answered, on
//! what they all found: the others' results are kept meanwhile.
//!
//! A Gate on the panel (`after` a round's outcome) opens only once the
//! round is closed with that outcome, on the round's latest Step, and its
//! Flow-given kind is what the Run's Standing Rules may answer; a
//! reviewer's question Gate is never put to them.

#[cfg(test)]
mod test;

use factory_records::domain::models::{Outcome, RunId, RunOutcome, StepKey};

use super::step::ask;
use super::work::Work;
use super::{Asking, FAILED_STEP_OUTPUT, Question, Questions};
use crate::domain::flow::Entry;
use crate::domain::gate::GateOpening;
use crate::domain::models::{StepEnding, StepReport, StepRequest, StepStart};
use crate::domain::review_rounds::{ReviewRoundClosing, ReviewRoundOpening};
use crate::domain::standing_rules::rule_fix_limit;

/// A review panel's visit: the entry, which visit of it, and the place of
/// its first Step.
pub(super) struct Panel<'a> {
    pub(super) run_id: RunId,
    pub(super) entry: &'a Entry,
    pub(super) visit: u16,
    pub(super) seq: u16,
    /// Whether its reviewers may ask the Run's owner questions, and how
    /// many each.
    pub(super) questions: Questions,
}

/// What a review round came to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Round {
    /// Its outcome, for the Flow to route.
    Routed(Outcome),
    /// The Run ended: cancelled, or before the round's Steps started.
    Ended(RunOutcome),
}

/// Run `panel`'s review round: open it, start a Step for each reviewer, in
/// the order the entry names them, run their work at once, record each
/// Step's end in that order, whichever ended first, put each reviewer's
/// question to the Run's owner and have it go on with the answer, and close
/// the round; then ask the panel's Gate when the round's outcome opens it.
/// The round's outcome, or the Gate's answer, comes back. `held` names the
/// Steps whose agents may still hold what their work used, for the
/// workflow to stop when the Run ends. `resume` gives the place of a Step
/// that goes on from a question, none once the Run took its most Steps.
pub(super) async fn review_round<W: Work>(
    work: &W,
    panel: &Panel<'_>,
    held: &mut Vec<StepRequest>,
    resume: &mut impl FnMut() -> Option<u16>,
) -> Result<Round, W::Failure> {
    // The round's latest Step, which the Run's latest Step is while its
    // Gate waits: its last reviewer's, or the last that went on from a
    // reviewer's question.
    let reviewers = u16::try_from(panel.entry.reviewers.len())
        .unwrap_or(u16::MAX)
        .max(1);
    let mut latest = panel.seq.saturating_add(reviewers - 1);
    let round = walk_round(work, panel, held, resume, &mut latest).await?;
    let Round::Routed(outcome) = round else {
        return Ok(round);
    };
    let Some(gate) = panel
        .entry
        .gate
        .as_ref()
        .filter(|gate| outcome == gate.opened_by())
    else {
        return Ok(Round::Routed(outcome));
    };

    let opening = GateOpening {
        key: StepKey {
            run_id: panel.run_id,
            seq: latest,
        },
        question: gate.question.clone(),
        answers: gate.answers.clone(),
    };
    // The rounds before this one each sent their Blockers to a fix.
    let rule = Asking {
        kind: gate.kind,
        fix_rounds: panel.visit.saturating_sub(1),
        fix_limit: rule_fix_limit(panel.entry.max_visits),
    };
    match ask(work, opening, Some(rule), held).await? {
        Some(answer) => Ok(Round::Routed(answer)),
        None => Ok(Round::Ended(RunOutcome::Cancelled)),
    }
}

/// Walk `panel`'s round up to its close, as [`review_round`] says, keeping
/// the place of its latest Step in `latest`.
async fn walk_round<W: Work>(
    work: &W,
    panel: &Panel<'_>,
    held: &mut Vec<StepRequest>,
    resume: &mut impl FnMut() -> Option<u16>,
    latest: &mut u16,
) -> Result<Round, W::Failure> {
    let opening = ReviewRoundOpening {
        run_id: panel.run_id,
        seq: panel.seq,
        round: panel.visit,
        reviewers: u16::try_from(panel.entry.reviewers.len()).unwrap_or(u16::MAX),
    };
    match work.open(opening).await {
        Ok(true) => {}
        // A round that could not open recorded why, and runs no Step.
        Ok(false) => return Ok(Round::Routed(Outcome::failed())),
        Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
        Err(_) => return Ok(Round::Routed(Outcome::failed())),
    }

    let requests: Vec<StepRequest> = panel
        .entry
        .reviewers
        .iter()
        .zip(panel.seq..)
        .map(|(role, seq)| {
            let key = StepKey {
                run_id: panel.run_id,
                seq,
            };
            panel.questions.offer(
                StepRequest::reviewer(key, panel.entry, panel.visit, role, panel.seq),
                0,
            )
        })
        .collect();
    for request in &requests {
        match work.start(request.clone()).await {
            Ok(StepStart::Started) => {}
            Ok(StepStart::RunEnded(outcome)) => return Ok(Round::Ended(outcome)),
            Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
            Err(error) => return Err(error),
        }
    }

    held.clone_from(&requests);
    let worked = work.run_all(&requests).await;

    let mut any_failed = false;
    let mut asking: Vec<(StepRequest, Question)> = Vec::new();
    for (request, worked) in requests.iter().zip(worked) {
        match end_reviewer(work, panel.questions, request, worked, held, &[]).await? {
            Reviewed::Ended(Ended::Done) => {}
            Reviewed::Ended(Ended::Failed) => any_failed = true,
            Reviewed::Asked(question) => asking.push((request.clone(), question)),
            Reviewed::RunEnded(outcome) => return Ok(Round::Ended(outcome)),
        }
    }

    // Each question waits for the owner's answer, and its reviewer goes on
    // with it, until it hands back its review, fails, or asks more than
    // its Step's questions: then the round fails, and the Run ends for a
    // person.
    // The Steps of a reviewer that asked stay held until it ends: the
    // Step that goes on claims their session only once its work starts.
    for (asking, question) in asking {
        let mut chain = vec![asking];
        let mut question = question;
        let mut asked: u8 = 0;
        loop {
            asked = asked.saturating_add(1);
            if asked > panel.questions.most {
                return stop_round(work, panel, RunOutcome::Manual).await;
            }
            let Some(last) = chain.last() else {
                break;
            };
            // Only the Run's owner answers a reviewer: no Standing Rule is
            // asked about a question Gate.
            match work.ask(question.opening(last.key), None).await {
                Ok(_) => {}
                Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
                Err(error) => {
                    for request in std::mem::take(held).iter().rev() {
                        work.stop(request).await;
                    }
                    return Err(error);
                }
            }
            let Some(seq) = resume() else {
                return stop_round(work, panel, RunOutcome::Blocked).await;
            };
            let key = StepKey {
                run_id: panel.run_id,
                seq,
            };
            *latest = (*latest).max(seq);
            let going_on = panel.questions.offer(last.going_on(key), asked);
            match work.start(going_on.clone()).await {
                Ok(StepStart::Started) => {}
                Ok(StepStart::RunEnded(outcome)) => return Ok(Round::Ended(outcome)),
                Err(_) if work.cancelled() => return Ok(Round::Ended(RunOutcome::Cancelled)),
                Err(error) => return Err(error),
            }
            held.push(going_on.clone());
            let worked = work.run(&going_on).await;
            match end_reviewer(work, panel.questions, &going_on, worked, held, &chain).await? {
                Reviewed::Ended(Ended::Done) => break,
                Reviewed::Ended(Ended::Failed) => {
                    any_failed = true;
                    break;
                }
                Reviewed::Asked(again) => {
                    chain.push(going_on);
                    question = again;
                }
                Reviewed::RunEnded(outcome) => return Ok(Round::Ended(outcome)),
            }
        }
    }

    let last = panel
        .entry
        .max_visits
        .is_some_and(|max| panel.visit >= u16::from(max.get()));
    let closing = ReviewRoundClosing {
        run_id: panel.run_id,
        seq: panel.seq,
        failed: any_failed,
        last,
    };
    match work.close(closing).await {
        Ok(outcome) => Ok(Round::Routed(outcome)),
        Err(_) if work.cancelled() => Ok(Round::Ended(RunOutcome::Cancelled)),
        Err(_) => Ok(Round::Routed(Outcome::failed())),
    }
}

/// How a reviewer's Step ended, once recorded.
enum Reviewed {
    /// It ended, and let go of its session or had its agent stopped.
    Ended(Ended),
    /// Its agent asked the Run's owner this question, and keeps its
    /// session for the Step that goes on with the answer.
    Asked(Question),
    /// The Run ended meanwhile.
    RunEnded(RunOutcome),
}

/// How a reviewer's Step that did not ask ended.
enum Ended {
    /// With its review.
    Done,
    /// Its work failed for good.
    Failed,
}

/// Record how the reviewer's Step `request` ended, from its work
/// `worked`: as for any Step, work that failed for good has its agent
/// stopped, and a Step that ended lets go of its session, unless it asked
/// the Run's owner a question. `asking` are the Steps whose question it
/// goes on from, in whose session it works: they are let go of, or
/// stopped, with it.
async fn end_reviewer<W: Work>(
    work: &W,
    questions: Questions,
    request: &StepRequest,
    worked: Result<StepReport, W::Failure>,
    held: &mut Vec<StepRequest>,
    asking: &[StepRequest],
) -> Result<Reviewed, W::Failure> {
    let (report, failed) = match worked {
        Ok(report) => (report, false),
        Err(_) if work.cancelled() => return Ok(Reviewed::RunEnded(RunOutcome::Cancelled)),
        Err(_) => (
            StepReport {
                outcome: Outcome::failed(),
                output: FAILED_STEP_OUTPUT.to_string(),
            },
            true,
        ),
    };
    let question = questions.asked(request, &report);
    let ending = StepEnding {
        key: request.key,
        report,
    };
    if let Err(error) = work.end(ending).await {
        if work.cancelled() {
            return Ok(Reviewed::RunEnded(RunOutcome::Cancelled));
        }
        return Err(error);
    }
    if let Some(question) = question {
        return Ok(Reviewed::Asked(question));
    }

    let ends = |kept: &StepRequest| {
        kept.key == request.key || asking.iter().any(|asked| asked.key == kept.key)
    };
    held.retain(|kept| !ends(kept));
    if failed {
        // The Step may have failed before it claimed the asking Steps'
        // session: stopping them stops it all the same.
        work.stop(request).await;
        for asked in asking.iter().rev() {
            work.stop(asked).await;
        }
        return Ok(Reviewed::Ended(Ended::Failed));
    }
    if let Err(error) = work.release(request.clone()).await {
        if work.cancelled() {
            held.extend(asking.iter().cloned());
            held.push(request.clone());
            return Ok(Reviewed::RunEnded(RunOutcome::Cancelled));
        }
        return Err(error);
    }
    Ok(Reviewed::Ended(Ended::Done))
}

/// Close `panel`'s round as failed, as a reviewer's question stopped it,
/// and end the Run `outcome`: the reviewers still held are stopped once the
/// Run's end is recorded.
async fn stop_round<W: Work>(
    work: &W,
    panel: &Panel<'_>,
    outcome: RunOutcome,
) -> Result<Round, W::Failure> {
    let closing = ReviewRoundClosing {
        run_id: panel.run_id,
        seq: panel.seq,
        failed: true,
        last: false,
    };
    match work.close(closing).await {
        Err(_) if work.cancelled() => Ok(Round::Ended(RunOutcome::Cancelled)),
        _ => Ok(Round::Ended(outcome)),
    }
}
