//! The replay measure: the Gate model run again over Gates people answered
//! themselves, to see how often it would have agreed with them and how
//! often it would have picked wrong, at each threshold.
//!
//! Two measures:
//! - replies: each Flow's Gate answered with a note, read as the person's
//!   words; the answer they gave with it is the one to agree with;
//! - the stand-in: each Gate of a stand-in kind (its answers are the plan's
//!   or the Blockers'), asked as the stand-in would be, with the owner's
//!   earlier answers to the same entry as its past answers.
//!
//! A pick counts at a threshold when the model was at least that sure:
//! it agrees when it is the person's answer, and is wrong otherwise. The
//! threshold to set is the lowest with no wrong pick among enough Gates the
//! model answered at it, once enough Gates were read; otherwise the measure
//! says so, and the conservative defaults stay. Only Gates a person
//! answered with a button are read: an answer the model read from their
//! words, or the stand-in's, is the model's own, not the truth to measure
//! it against. The stand-in is asked with the Run's plan, as it is when it
//! answers.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use factory_records::domain::gate_replies::{AnsweredGate, PastAnswer};
use factory_records::domain::models::{GateKind, Outcome, RunFlow, RunId};
use factory_records::domain::standing_rules::RuleGateKind;

use super::{percent, reply_question};
use crate::domain::built_in_flows::walked_flow;
use crate::domain::ports::{GateModel, ModelPick};
use crate::domain::stand_in::{PAST_ANSWERS_READ, StandInContext, stand_in_question};
use crate::domain::standing_rules::offers_kind_answers;

/// The thresholds the measure reports, in percent.
pub const THRESHOLDS: [u8; 7] = [50, 60, 70, 80, 85, 90, 95];

/// The fewest Gates a measure reads before its threshold is trusted.
pub const ENOUGH_GATES: usize = 30;

/// The fewest Gates the model must have answered at a threshold for that
/// threshold to be set from: no wrong pick among a handful says little.
pub const ENOUGH_DECIDED: usize = 20;

/// How the model did at one threshold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AtThreshold {
    /// The threshold, in percent.
    pub threshold: u8,
    /// The Gates it would have answered.
    pub decided: usize,
    /// Those it answered as the person did.
    pub agreed: usize,
    /// Those it answered otherwise.
    pub wrong: usize,
}

/// How the model did over one set of Gates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Measure {
    /// The Gates read.
    pub gates: usize,
    /// The calls that failed: they decide nothing.
    pub failed: usize,
    /// How it did at each of [`THRESHOLDS`].
    pub thresholds: Vec<AtThreshold>,
}

impl Measure {
    /// The lowest threshold at which the model picked nothing wrong and
    /// answered at least [`ENOUGH_DECIDED`] Gates, once [`ENOUGH_GATES`]
    /// were read; none when too few were, or no threshold qualifies.
    pub fn threshold(&self) -> Option<u8> {
        if self.gates < ENOUGH_GATES {
            return None;
        }
        self.thresholds
            .iter()
            .find(|at| at.wrong == 0 && at.decided >= ENOUGH_DECIDED)
            .map(|at| at.threshold)
    }

    /// Count `pick` against `answer`, the person's.
    fn count(&mut self, pick: Option<&ModelPick>, answer: &Outcome) {
        self.gates += 1;
        let Some(pick) = pick else {
            self.failed += 1;
            return;
        };
        let Some(choice) = &pick.choice else {
            return;
        };
        let confidence = percent(pick.confidence);
        for at in &mut self.thresholds {
            if confidence < at.threshold {
                continue;
            }
            at.decided += 1;
            if choice == answer {
                at.agreed += 1;
            } else {
                at.wrong += 1;
            }
        }
    }
}

/// A measure with one empty row per threshold, before any Gate is read.
fn empty_measure() -> Measure {
    Measure {
        thresholds: THRESHOLDS
            .iter()
            .map(|threshold| AtThreshold {
                threshold: *threshold,
                ..AtThreshold::default()
            })
            .collect(),
        ..Measure::default()
    }
}

/// Both measures over the same Gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayReport {
    /// The model reading people's notes as their answers.
    pub replies: Measure,
    /// The model standing in for the owners.
    pub stand_in: Measure,
}

/// The kind of `gate`, as its Run's own Flow names it, when the stand-in
/// may answer Gates of that kind: never a merge Gate, nor a Gate of a Run
/// whose Flow is not known.
pub fn stand_in_kind(gate: &AnsweredGate) -> Option<RuleGateKind> {
    let flow = walked_flow(
        gate.run_id,
        RunFlow {
            name: gate.flow.clone(),
            definition: gate.flow_definition.clone(),
        },
    )?;
    let kind = flow
        .entries
        .iter()
        .find(|entry| entry.id == gate.entry)?
        .gate
        .as_ref()?
        .kind?;
    let routine = matches!(kind, RuleGateKind::Plan | RuleGateKind::Blockers);
    (routine && offers_kind_answers(kind, &gate.answers)).then_some(kind)
}

/// Run `model` over `gates`, oldest first, as both measures say; each call
/// is billed to the Gate's owner.
pub async fn replay<M: GateModel>(model: &M, gates: &[AnsweredGate]) -> ReplayReport {
    let mut replies = empty_measure();
    let mut stand_in = empty_measure();
    // Each owner's answers to an entry so far, newest first, as the
    // stand-in reads its past answers.
    let mut past: BTreeMap<(String, String), Vec<PastAnswer>> = BTreeMap::new();

    for gate in gates.iter().filter(|gate| gate.kind == GateKind::Flow) {
        let note = gate
            .note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty());
        if let Some(note) = note
            && gate.answers.len() > 1
        {
            let question = reply_question(&gate.question, &gate.answers, note);
            let pick = model.pick(&gate.owner, &question).await.ok();
            replies.count(pick.as_ref(), &gate.answer);
        }

        let key = (gate.owner.to_string(), gate.entry.as_str().to_string());
        if stand_in_kind(gate).is_some() {
            let earlier = past.get(&key).map(Vec::as_slice).unwrap_or_default();
            let context = StandInContext {
                question: &gate.question,
                answers: &gate.answers,
                request: &gate.request,
                output: &gate.output,
                plan: gate.plan.as_deref(),
                past: earlier,
            };
            let pick = model
                .pick(&gate.owner, &stand_in_question(&context))
                .await
                .ok();
            stand_in.count(pick.as_ref(), &gate.answer);
        }

        let answers = past.entry(key).or_default();
        answers.insert(
            0,
            PastAnswer {
                question: gate.question.clone(),
                answer: gate.answer.clone(),
                note: gate.note.clone(),
            },
        );
        answers.truncate(usize::from(PAST_ANSWERS_READ));
    }
    ReplayReport { replies, stand_in }
}

/// `report`, as the replay command prints it.
pub fn describe(report: &ReplayReport) -> String {
    let mut text = String::new();
    for (name, measure) in [
        ("Replies (notes read as answers)", &report.replies),
        ("Stand-in (plan and Blockers Gates)", &report.stand_in),
    ] {
        text.push_str(&format!(
            "{name}: {} gates read, {} calls failed\n  threshold  decided  agreed  wrong\n",
            measure.gates, measure.failed
        ));
        for at in &measure.thresholds {
            text.push_str(&format!(
                "  {:>8}%  {:>7}  {:>6}  {:>5}\n",
                at.threshold, at.decided, at.agreed, at.wrong
            ));
        }
        match measure.threshold() {
            Some(threshold) => text.push_str(&format!(
                "  Lowest threshold with no wrong pick: {threshold}%\n\n"
            )),
            None if measure.gates < ENOUGH_GATES => text.push_str(&format!(
                "  Too few gates to set a threshold from (fewer than {ENOUGH_GATES}): keep the \
                 conservative default.\n\n"
            )),
            None => text.push_str(&format!(
                "  No threshold had no wrong pick among at least {ENOUGH_DECIDED} answered \
                 gates: keep the conservative default.\n\n"
            )),
        }
    }
    text
}

/// The Runs `gates` come from, for the report's reader.
pub fn runs(gates: &[AnsweredGate]) -> usize {
    let mut runs: Vec<RunId> = gates.iter().map(|gate| gate.run_id).collect();
    runs.sort();
    runs.dedup();
    runs.len()
}
