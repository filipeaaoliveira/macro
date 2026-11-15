//! The state machine a Flow describes, with no Temporal in it: which entry a
//! Run runs next, and where a Step's outcome takes it. The workflow
//! (`crate::inbound::temporal`) drives it and runs the Steps; replaying the
//! same outcomes always walks the same way.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use factory_records::domain::models::{EntryId, Outcome, RunOutcome};

use crate::domain::flow::{Condition, Entry, FactName, Flow, Target, Transition};

/// Where a Run is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// At the entry with this index, about to run it or running it.
    Entry(usize),
    /// Ended.
    End(RunOutcome),
}

/// A Run's state as it walks its Flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunState {
    position: Position,
    visits: BTreeMap<EntryId, u16>,
    facts: BTreeMap<FactName, bool>,
}

impl RunState {
    /// The value of `fact`; a fact never set is false.
    pub fn fact(&self, fact: &FactName) -> bool {
        self.facts.get(fact).copied().unwrap_or(false)
    }
}

/// What the Run does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextStep<'a> {
    /// Run this entry's Step; it is the entry's `visit`.
    Run {
        /// The entry.
        entry: &'a Entry,
        /// Which visit of the entry this is, from 1.
        visit: u16,
    },
    /// The Run has ended.
    End(RunOutcome),
}

/// Why a Run cannot go on. Each is a Flow that does not hold together at run
/// time; the Run ends failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InterpreterError {
    /// An outcome has no transition, and its entry has no `next` and no entry
    /// below it.
    #[error("entry {entry} has nowhere to go after {outcome}")]
    NoWayOn {
        /// The entry.
        entry: EntryId,
        /// The outcome.
        outcome: Outcome,
    },
    /// A skipped entry has no `next` and no entry below it.
    #[error("entry {0} is skipped and has nowhere to go")]
    NoWayPast(EntryId),
    /// A transition names an entry the Flow does not have.
    #[error("the flow has no entry {0}")]
    UnknownEntry(EntryId),
    /// Every entry the Run reaches is skipped or has run out of visits.
    #[error("the flow skips or exhausts every entry it reaches")]
    NoStepToRun,
    /// An entry ran more times than a visit number holds.
    #[error("entry {0} ran too many times")]
    TooManyVisits(EntryId),
    /// An outcome was recorded while the Run was not at an entry.
    #[error("the run has ended")]
    Ended,
}

/// A Flow, read as a state machine.
pub struct Interpreter<'a> {
    flow: &'a Flow,
}

impl<'a> Interpreter<'a> {
    /// The state machine of `flow`.
    pub fn new(flow: &'a Flow) -> Self {
        Self { flow }
    }

    /// A new Run's state: at the first entry, with the Flow's starting facts.
    pub fn start(&self) -> RunState {
        RunState {
            position: Position::Entry(0),
            visits: BTreeMap::new(),
            facts: self.flow.facts.clone(),
        }
    }

    /// Move the Run past the entries it skips or that ran out of visits, to
    /// the next Step it runs or to its end, and count that Step's visit.
    pub fn next_step(&self, state: &mut RunState) -> Result<NextStep<'a>, InterpreterError> {
        // Each move that runs no Step lands on another entry or the end; more
        // moves than the Flow has entries can only be a loop that never runs
        // a Step.
        for _ in 0..=self.flow.entries.len() {
            let index = match state.position {
                Position::End(outcome) => return Ok(NextStep::End(outcome)),
                Position::Entry(index) => index,
            };
            let entry = self.entry(index)?;

            if !self.all_hold(&entry.when, state) {
                state.position = self.position(&self.default_target(index, None)?)?;
                continue;
            }

            let visits = state.visits.get(&entry.id).copied().unwrap_or(0);
            if let Some(max) = entry.max_visits
                && visits >= u16::from(max.get())
            {
                let blocked = Transition::to(Target::End {
                    outcome: RunOutcome::Blocked,
                });
                let transition = self.first_holding(&entry.on_exhausted, state);
                self.apply(transition.unwrap_or(&blocked), state)?;
                continue;
            }

            let visit = visits
                .checked_add(1)
                .ok_or_else(|| InterpreterError::TooManyVisits(entry.id.clone()))?;
            state.visits.insert(entry.id.clone(), visit);
            return Ok(NextStep::Run { entry, visit });
        }
        Err(InterpreterError::NoStepToRun)
    }

    /// Take the Run where `outcome`, of the Step it just ran, leads: the first
    /// transition for it whose conditions hold, else the entry's `next`. A
    /// failed Step the Flow does not route ends the Run failed.
    pub fn record(&self, state: &mut RunState, outcome: &Outcome) -> Result<(), InterpreterError> {
        let Position::Entry(index) = state.position else {
            return Err(InterpreterError::Ended);
        };
        let entry = self.entry(index)?;

        let routed = entry
            .on
            .get(outcome)
            .and_then(|transitions| self.first_holding(transitions, state));
        if let Some(transition) = routed {
            return self.apply(transition, state);
        }

        if *outcome == Outcome::failed() {
            state.position = Position::End(RunOutcome::Failed);
            return Ok(());
        }
        state.position = self.position(&self.default_target(index, Some(outcome))?)?;
        Ok(())
    }

    fn entry(&self, index: usize) -> Result<&'a Entry, InterpreterError> {
        self.flow
            .entries
            .get(index)
            .ok_or(InterpreterError::NoStepToRun)
    }

    fn all_hold(&self, conditions: &[Condition], state: &RunState) -> bool {
        conditions
            .iter()
            .all(|condition| state.fact(&condition.fact) == condition.equals)
    }

    fn first_holding<'t>(
        &self,
        transitions: &'t [Transition],
        state: &RunState,
    ) -> Option<&'t Transition> {
        transitions
            .iter()
            .find(|transition| self.all_hold(&transition.when, state))
    }

    /// Where the entry at `index` goes when nothing routes it: its `next`,
    /// else the entry below it.
    fn default_target(
        &self,
        index: usize,
        outcome: Option<&Outcome>,
    ) -> Result<Target, InterpreterError> {
        let entry = self.entry(index)?;
        if let Some(next) = &entry.next {
            return Ok(next.clone());
        }
        match self.flow.entries.get(index + 1) {
            Some(below) => Ok(Target::Entry {
                entry: below.id.clone(),
            }),
            None => match outcome {
                Some(outcome) => Err(InterpreterError::NoWayOn {
                    entry: entry.id.clone(),
                    outcome: outcome.clone(),
                }),
                None => Err(InterpreterError::NoWayPast(entry.id.clone())),
            },
        }
    }

    fn position(&self, target: &Target) -> Result<Position, InterpreterError> {
        match target {
            Target::End { outcome } => Ok(Position::End(*outcome)),
            Target::Entry { entry } => self
                .flow
                .entries
                .iter()
                .position(|listed| listed.id == *entry)
                .map(Position::Entry)
                .ok_or_else(|| InterpreterError::UnknownEntry(entry.clone())),
        }
    }

    /// A transition's changes: the facts it sets, the visits it resets, and
    /// the Run's new position.
    fn apply(&self, transition: &Transition, state: &mut RunState) -> Result<(), InterpreterError> {
        let position = self.position(&transition.to)?;
        for (fact, value) in &transition.set {
            state.facts.insert(fact.clone(), *value);
        }
        for entry in &transition.reset {
            state.visits.remove(entry);
        }
        state.position = position;
        Ok(())
    }
}
