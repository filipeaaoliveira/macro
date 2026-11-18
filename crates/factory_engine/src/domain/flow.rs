//! A Flow: the entries a Run walks, the Step each one runs, and where each
//! Step's outcome takes the Run. Flows are data, written as JSON.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU8;

use factory_records::domain::models::{
    EntryId, InvalidName, MAX_QUESTION_BYTES, Outcome, Role, RunOutcome, checked_name,
};
use factory_records::domain::standing_rules::RuleGateKind;

use crate::domain::standing_rules::{kind_answers, offers_kind_answers};
use serde::{Deserialize, Serialize};

/// A fact a Flow keeps while a Run goes: a yes or no its transitions set and
/// its conditions read, such as whether a review panel has run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FactName(String);

impl TryFrom<String> for FactName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("fact", value).map(Self)
    }
}

impl From<FactName> for String {
    fn from(name: FactName) -> Self {
        name.0
    }
}

impl fmt::Display for FactName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of the work a Step does, such as `reply`. The worker that runs a
/// Flow's Steps says which names it serves.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct StepName(String);

impl StepName {
    /// The name as written in the Flow.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for StepName {
    type Error = InvalidName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        checked_name("step", value).map(Self)
    }
}

impl From<StepName> for String {
    fn from(name: StepName) -> Self {
        name.0
    }
}

impl fmt::Display for StepName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a worker does a Step it serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepUse {
    /// It does the work itself.
    Work,
    /// An agent does it, filling the Role its entry names.
    Agent,
}

/// Where a transition takes the Run: to an entry, or to the Run's end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    /// Run this entry next.
    Entry {
        /// The entry.
        entry: EntryId,
    },
    /// End the Run.
    End {
        /// How the Run ends.
        outcome: RunOutcome,
    },
}

/// A condition on one of the Run's facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Condition {
    /// The fact it reads.
    pub fact: FactName,
    /// The value the fact must have.
    pub equals: bool,
}

/// One way out of an entry. It applies when all its conditions hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// Conditions on the Run's facts; none always holds.
    #[serde(default)]
    pub when: Vec<Condition>,
    /// Where it takes the Run.
    pub to: Target,
    /// Facts it sets.
    #[serde(default)]
    pub set: BTreeMap<FactName, bool>,
    /// Entries whose visits it counts from zero again.
    #[serde(default)]
    pub reset: Vec<EntryId>,
}

impl Transition {
    /// A transition that only moves the Run to `to`.
    pub fn to(to: Target) -> Self {
        Self {
            when: Vec::new(),
            to,
            set: BTreeMap::new(),
            reset: Vec::new(),
        }
    }
}

/// A question an entry puts to a person once its Step ends `done`, or, for
/// a review panel, once a round ends with the outcome it names. The Run
/// waits for the answer, unless a Standing Rule gives it, and the answer
/// is the outcome the Flow routes. A notify-only Gate (`notify_only`) only
/// tells the person: the Run does not wait, and the Flow routes the
/// outcome that opened it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateDefinition {
    /// The question, shown with the Step's output.
    pub question: String,
    /// The answers a person may give; none for a notify-only Gate.
    #[serde(default)]
    pub answers: Vec<Outcome>,
    /// The outcome that opens it: `done` when absent. A review panel's Gate
    /// names the round's outcome it asks about, such as `changes`. Left
    /// out of a Flow that has none, so Runs that carry such a Flow read the
    /// same.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<Outcome>,
    /// The kind of Gate it is, whose Standing Rule may answer it; none for a
    /// Gate no rule answers. A Gate of a kind offers exactly its kind's
    /// answers. Left out of a Flow that has none, as `after` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<RuleGateKind>,
    /// Whether only a person answers it: the stand-in never does, whatever
    /// the Run's Orchestrator Definition says. Left out of a Flow that has
    /// none, as `after` is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub person_only: bool,
    /// Whether it only tells the person its question and lets the Run go
    /// on: it offers no answers, has no kind and is not `person_only`.
    /// Left out of a Flow that has none, as `after` is.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notify_only: bool,
}

impl GateDefinition {
    /// The outcome of the entry's Step, or of its review round, that opens
    /// the Gate.
    pub fn opened_by(&self) -> Outcome {
        self.after.clone().unwrap_or_else(Outcome::done)
    }
}

/// One entry of a Flow: a Step to run, when, how often, and where each of its
/// outcomes goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// The entry's id, unique in its Flow.
    pub id: EntryId,
    /// The Step it runs.
    pub step: StepName,
    /// The Role an agent fills to do the Step, when an agent does it: the
    /// factory's agent definition says which Agents fill it.
    #[serde(default)]
    pub role: Option<Role>,
    /// The Roles of a review panel, in place of `role`: the entry's Step
    /// runs once for each, at once, each in a Step of its own, as one
    /// review round, whose outcome the Flow routes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reviewers: Vec<Role>,
    /// Conditions on the Run's facts; when one does not hold the entry is
    /// skipped and the Run goes to its `next`.
    #[serde(default)]
    pub when: Vec<Condition>,
    /// How many times the entry may run before the Run takes `on_exhausted`.
    #[serde(default)]
    pub max_visits: Option<NonZeroU8>,
    /// Where the Run goes once the entry ran `max_visits` times: the first
    /// transition whose conditions hold, else the Run ends blocked.
    #[serde(default)]
    pub on_exhausted: Vec<Transition>,
    /// Where an outcome with no transition in `on`, or a skip, goes; the
    /// entry below when absent.
    #[serde(default)]
    pub next: Option<Target>,
    /// Where each outcome goes: the first transition whose conditions hold.
    #[serde(default)]
    pub on: BTreeMap<Outcome, Vec<Transition>>,
    /// The question the entry asks once its Step ends `done` (or the
    /// outcome its `after` names); its answer is then the outcome `on`
    /// routes.
    #[serde(default)]
    pub gate: Option<GateDefinition>,
}

/// A Flow definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Flow {
    /// What the Flow does, for a person.
    #[serde(default)]
    pub description: String,
    /// Whether a Run of it works on a GitHub repository: one is started with
    /// the repository, and the others with none.
    #[serde(default)]
    pub repository: bool,
    /// The facts the Flow keeps, with their values when a Run starts.
    #[serde(default)]
    pub facts: BTreeMap<FactName, bool>,
    /// The entries, in order. A Run starts at the first.
    pub entries: Vec<Entry>,
    /// How many questions an agent Step may put to the Run's owner, each
    /// ending the Step on a question Gate the same Role goes on from with
    /// the answer; the next question after them ends the Run `manual`, for
    /// a person. None, or zero, for a Flow whose agents ask nothing, as
    /// every Run started before questions has: their workflows replay as
    /// before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub questions: Option<u8>,
}

/// What is wrong with a Flow definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    /// The definition is not a Flow.
    #[error("the flow does not read: {0}")]
    Unreadable(String),
    /// The Flow has no entries.
    #[error("the flow has no entries")]
    NoEntries,
    /// Two entries share an id.
    #[error("entry {0} appears twice")]
    DuplicateEntry(EntryId),
    /// A target, a reset or a cursor names an entry the Flow does not have.
    #[error("entry {from} names entry {to}, which the flow does not have")]
    UnknownEntry {
        /// The entry that names it.
        from: EntryId,
        /// The entry it names.
        to: EntryId,
    },
    /// A condition or a transition names a fact the Flow does not declare.
    #[error("entry {entry} names fact {fact}, which the flow does not declare")]
    UnknownFact {
        /// The entry that names it.
        entry: EntryId,
        /// The fact it names.
        fact: FactName,
    },
    /// A notify-only Gate offers answers, names a kind or is kept for a
    /// person: it takes no answer.
    #[error(
        "entry {0}'s notify-only gate offers answers, a kind or person_only: it takes no answer"
    )]
    NotifyOnlyGateAnswers(EntryId),
    /// A Gate has no question, a question too long to keep, or no answers.
    #[error(
        "entry {0} has a gate with no question, a question over {MAX_QUESTION_BYTES} bytes, or no answers"
    )]
    IncompleteGate(EntryId),
    /// A Gate offers one answer twice.
    #[error("entry {entry}'s gate offers {answer} twice")]
    DuplicateAnswer {
        /// The entry.
        entry: EntryId,
        /// The answer.
        answer: Outcome,
    },
    /// An agent does the entry's Step, and the entry names no Role for it to
    /// fill.
    #[error("an agent does entry {0}'s step, and it names no role")]
    NoRole(EntryId),
    /// A review panel's entry also names a single Role, or a Gate that does
    /// not say which round's outcome opens it.
    #[error("entry {0} names reviewers, and a role or a gate with no `after` beside them")]
    ReviewersAndRole(EntryId),
    /// A review panel bounded in rounds says nowhere for its last round's
    /// unresolved Blockers to go.
    #[error("entry {0} bounds its review rounds and routes no `unresolved`")]
    NoUnresolvedRoute(EntryId),
    /// A review panel's Gate opens after an outcome no round ends with.
    #[error("entry {0}'s gate opens after an outcome no review round ends with")]
    GateAfterNoRoundOutcome(EntryId),
    /// A Flow's Gate opens after an agent's question, which opens a
    /// question Gate of its own that only the Run's owner answers.
    #[error("entry {0}'s gate opens after `question`, which only an agent's question gate takes")]
    GateAfterQuestion(EntryId),
    /// A Flow's Gate opens after `no_result`, which opens a Gate of its own
    /// that only the Run's owner answers.
    #[error(
        "entry {0}'s gate opens after `no_result`, which only the gate of an agent that stopped without a result takes"
    )]
    GateAfterNoResult(EntryId),
    /// A Gate of a kind offers other answers than its kind's.
    #[error("entry {entry}'s {kind} gate must offer exactly {answers:?}")]
    GateKindAnswers {
        /// The entry.
        entry: EntryId,
        /// The Gate's kind.
        kind: RuleGateKind,
        /// The answers a Gate of the kind offers.
        answers: [&'static str; 2],
    },
    /// A review panel names one Role twice.
    #[error("entry {entry} names reviewer {role} twice")]
    DuplicateReviewer {
        /// The entry.
        entry: EntryId,
        /// The Role.
        role: Role,
    },
    /// A Gate on an entry that runs `ready_to_merge` or `merge` is not a
    /// merge Gate, or a merge Gate is on an entry that does not run
    /// `ready_to_merge`: only a merge Gate asks to merge, so only its rule,
    /// and never the Orchestrator, approves a merge.
    #[error(
        "entry {0}: a gate on ready_to_merge or merge must be of kind merge, and a merge gate must be on ready_to_merge"
    )]
    MergeGateKind(EntryId),
    /// An entry the worker does itself names a Role or reviewers.
    #[error("entry {0} runs a step no agent does, and names a role or reviewers")]
    RoleOnWork(EntryId),
    /// A merge Gate opens after another outcome than `done`: it would ask
    /// to merge a head whose checks did not pass, and leave `done`, which
    /// nothing routes, to fall down into the merge.
    #[error("entry {0}'s merge gate must open after `done`")]
    MergeGateAfter(EntryId),
    /// A way into an entry that runs `merge` does not go through a merge
    /// Gate's approval: every merge needs a person's, or the merge rule's,
    /// answer first.
    #[error("entry {from} leads to the merge at entry {to} without a merge gate's approval")]
    UnguardedMerge {
        /// The entry the way starts from; the merge entry itself when the
        /// Run would start there.
        from: EntryId,
        /// The entry that runs `merge`.
        to: EntryId,
    },
    /// An entry runs a Step the worker does not serve.
    #[error("entry {entry} runs step {step}, which no worker serves")]
    UnknownStep {
        /// The entry.
        entry: EntryId,
        /// Its Step.
        step: StepName,
    },
}

impl Flow {
    /// Read a Flow from its JSON definition and check that it holds together:
    /// unique entries, every entry, fact and Step it names known (`serves`
    /// says how a worker does each Step it serves), and a Role on every
    /// entry an agent does, or the Roles of its review panel.
    pub fn parse(
        definition: serde_json::Value,
        serves: impl Fn(&StepName) -> Option<StepUse>,
    ) -> Result<Self, FlowError> {
        let flow: Flow = serde_json::from_value(definition)
            .map_err(|error| FlowError::Unreadable(error.to_string()))?;
        flow.check(serves)?;
        Ok(flow)
    }

    /// Let its agent Steps ask the Run's owner up to `questions` questions
    /// each.
    pub fn allow_questions(&mut self, questions: u8) {
        self.questions = Some(questions);
    }

    /// Bound each review panel to `rounds` rounds, in place of the
    /// `max_visits` the Flow gives it.
    pub fn bound_review_rounds(&mut self, rounds: NonZeroU8) {
        let panels = self
            .entries
            .iter_mut()
            .filter(|entry| !entry.reviewers.is_empty());
        for panel in panels {
            panel.max_visits = Some(rounds);
        }
    }

    fn check(&self, serves: impl Fn(&StepName) -> Option<StepUse>) -> Result<(), FlowError> {
        if self.entries.is_empty() {
            return Err(FlowError::NoEntries);
        }

        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            if !ids.insert(&entry.id) {
                return Err(FlowError::DuplicateEntry(entry.id.clone()));
            }
        }

        for entry in &self.entries {
            match serves(&entry.step) {
                None => {
                    return Err(FlowError::UnknownStep {
                        entry: entry.id.clone(),
                        step: entry.step.clone(),
                    });
                }
                Some(StepUse::Agent) if entry.role.is_none() && entry.reviewers.is_empty() => {
                    return Err(FlowError::NoRole(entry.id.clone()));
                }
                Some(_) => {}
            }
            check_reviewers(entry)?;

            if let Some(gate) = &entry.gate {
                check_gate(&entry.id, gate)?;
            }

            let transitions = entry.on.values().flatten().chain(&entry.on_exhausted);
            let mut targets: Vec<&Target> = transitions.clone().map(|t| &t.to).collect();
            targets.extend(&entry.next);
            let mut named: Vec<&EntryId> = transitions.clone().flat_map(|t| &t.reset).collect();
            for target in targets {
                if let Target::Entry { entry } = target {
                    named.push(entry);
                }
            }
            if let Some(unknown) = named.into_iter().find(|id| !ids.contains(id)) {
                return Err(FlowError::UnknownEntry {
                    from: entry.id.clone(),
                    to: unknown.clone(),
                });
            }

            let read = transitions.clone().flat_map(|t| &t.when).map(|c| &c.fact);
            let set = transitions.flat_map(|t| t.set.keys());
            let mut facts = entry.when.iter().map(|c| &c.fact).chain(read).chain(set);
            if let Some(unknown) = facts.find(|fact| !self.facts.contains_key(fact)) {
                return Err(FlowError::UnknownFact {
                    entry: entry.id.clone(),
                    fact: unknown.clone(),
                });
            }
        }
        Ok(())
    }

    /// Fails when an entry whose Step a worker does itself (`StepUse::Work`:
    /// `merge`, `ready_to_merge`, `publish`...) names a Role or a review
    /// panel, which no agent would fill. A Factory Definition's Flows are
    /// held to it; Flows read with every Step taken as work are not.
    pub fn check_work_roles(
        &self,
        serves: impl Fn(&StepName) -> Option<StepUse>,
    ) -> Result<(), FlowError> {
        let named = self.entries.iter().find(|entry| {
            let roles = entry.role.is_some() || !entry.reviewers.is_empty();
            roles && serves(&entry.step) == Some(StepUse::Work)
        });
        match named {
            Some(entry) => Err(FlowError::RoleOnWork(entry.id.clone())),
            None => Ok(()),
        }
    }

    /// Fails unless only a merge Gate asks to merge, and every way into an
    /// entry that runs `merge` goes through its approval. A Factory
    /// Definition's Flows are held to it, and the built-in Flows keep to
    /// it; Flows Runs carried before it are not read again.
    ///
    /// Only a merge Gate asks to merge, and every way into an entry that
    /// runs `merge` goes through its approval: an explicit transition only
    /// under its approving answer, and the way down from the entry above
    /// only from a merge Gate's entry that routes every other outcome. A
    /// Factory Definition's Flow could otherwise merge with no one's
    /// approval, or label a merge Gate as another kind that the
    /// Orchestrator or a plan rule approves.
    pub fn check_merges(&self) -> Result<(), FlowError> {
        let runs = |entry: &Entry, step: &str| entry.step.as_str() == step;
        let merges: BTreeSet<&EntryId> = self
            .entries
            .iter()
            .filter(|entry| runs(entry, MERGE_STEP))
            .map(|entry| &entry.id)
            .collect();
        let into_merge = |target: &Target| match target {
            Target::Entry { entry } => merges.get(entry).copied(),
            Target::End { .. } => None,
        };
        let [approve, _] = kind_answers(RuleGateKind::Merge);

        if let Some(first) = self.entries.first()
            && runs(first, MERGE_STEP)
        {
            return Err(FlowError::UnguardedMerge {
                from: first.id.clone(),
                to: first.id.clone(),
            });
        }

        for (index, entry) in self.entries.iter().enumerate() {
            let at_merge = runs(entry, READY_TO_MERGE_STEP) || runs(entry, MERGE_STEP);
            let merge_gate = entry
                .gate
                .as_ref()
                .is_some_and(|gate| gate.kind == Some(RuleGateKind::Merge));
            let wrong_kind = at_merge && entry.gate.is_some() && !merge_gate;
            if wrong_kind || (merge_gate && !runs(entry, READY_TO_MERGE_STEP)) {
                return Err(FlowError::MergeGateKind(entry.id.clone()));
            }
            // `ready_to_merge` ends `done` on the head whose checks passed:
            // only then may a merge Gate ask, as its rule assumes.
            let opens_after_done = entry
                .gate
                .as_ref()
                .is_none_or(|gate| gate.opened_by() == Outcome::done());
            if merge_gate && !opens_after_done {
                return Err(FlowError::MergeGateAfter(entry.id.clone()));
            }
            let unguarded = |to: &EntryId| FlowError::UnguardedMerge {
                from: entry.id.clone(),
                to: to.clone(),
            };

            for (outcome, transitions) in &entry.on {
                let approved = merge_gate && outcome.as_str() == approve;
                let leads = transitions.iter().find_map(|t| into_merge(&t.to));
                if let Some(to) = leads
                    && !approved
                {
                    return Err(unguarded(to));
                }
            }
            let exhausted = entry.on_exhausted.iter().find_map(|t| into_merge(&t.to));
            let next = entry.next.as_ref().and_then(into_merge);
            if let Some(to) = exhausted.or(next) {
                return Err(unguarded(to));
            }

            // With no `next`, a skip and an outcome nothing routes go to
            // the entry below: above a merge, only a merge Gate's entry
            // that is never skipped and routes, with no condition, every
            // outcome `ready_to_merge` ends with and both its answers.
            let Some(below) = self.entries.get(index + 1) else {
                continue;
            };
            if entry.next.is_some() || !runs(below, MERGE_STEP) {
                continue;
            }
            let routed = |outcome: &str| {
                entry.on.iter().any(|(routes, transitions)| {
                    routes.as_str() == outcome && transitions.iter().any(|t| t.when.is_empty())
                })
            };
            let covered = merge_gate
                && entry.when.is_empty()
                && READY_TO_MERGE_OUTCOMES
                    .iter()
                    .all(|outcome| routed(outcome))
                && kind_answers(RuleGateKind::Merge)
                    .iter()
                    .all(|answer| routed(answer));
            if !covered {
                return Err(unguarded(&below.id));
            }
        }
        Ok(())
    }
}

/// The Step that merges a pull request.
const MERGE_STEP: &str = "merge";

/// The Step whose Gate asks to merge a pull request.
const READY_TO_MERGE_STEP: &str = "ready_to_merge";

/// The outcomes `ready_to_merge` ends with besides `done`, which opens its
/// Gate (`pull_request_steps.rs`).
const READY_TO_MERGE_OUTCOMES: [&str; 3] = ["changed", "merged", "closed"];

/// A review panel names each of its Roles once, and no single Role beside
/// them. Its outcome is the round's; a Gate on it asks about one of the
/// round's outcomes (`after`), since a round never ends `done`.
fn check_reviewers(entry: &Entry) -> Result<(), FlowError> {
    if entry.reviewers.is_empty() {
        return Ok(());
    }
    let unopened = entry.gate.as_ref().is_some_and(|gate| gate.after.is_none());
    if entry.role.is_some() || unopened {
        return Err(FlowError::ReviewersAndRole(entry.id.clone()));
    }
    // A round ends `pass`, `changes`, `unresolved` or `failed`: a Gate
    // after anything else would never open.
    let after = entry.gate.as_ref().and_then(|gate| gate.after.as_ref());
    if after.is_some_and(|after| !ROUND_OUTCOMES.contains(&after.as_str())) {
        return Err(FlowError::GateAfterNoRoundOutcome(entry.id.clone()));
    }

    // A last round that still finds Blockers routes `unresolved`: a panel
    // bounded in rounds says where that goes.
    let unresolved = Outcome::try_from("unresolved".to_string())
        .map_err(|error| FlowError::Unreadable(error.to_string()))?;
    if entry.max_visits.is_some() && !entry.on.contains_key(&unresolved) {
        return Err(FlowError::NoUnresolvedRoute(entry.id.clone()));
    }

    let mut named = BTreeSet::new();
    if let Some(twice) = entry.reviewers.iter().find(|role| !named.insert(*role)) {
        return Err(FlowError::DuplicateReviewer {
            entry: entry.id.clone(),
            role: twice.clone(),
        });
    }
    Ok(())
}

/// The outcomes a review round ends with.
const ROUND_OUTCOMES: [&str; 4] = ["pass", "changes", "unresolved", "failed"];

fn check_gate(entry: &EntryId, gate: &GateDefinition) -> Result<(), FlowError> {
    // A Step that ends `question` opens a question Gate, which only the
    // Run's owner answers: a Flow's Gate after it would put the agent's
    // question to whatever answers the Flow's Gates, Standing Rules
    // included.
    if gate
        .after
        .as_ref()
        .is_some_and(|after| *after == Outcome::question())
    {
        return Err(FlowError::GateAfterQuestion(entry.clone()));
    }
    // Nor after `no_result`: the Gate of an agent that stopped without a
    // result is the Run's owner's alone.
    if gate
        .after
        .as_ref()
        .is_some_and(|after| *after == Outcome::no_result())
    {
        return Err(FlowError::GateAfterNoResult(entry.clone()));
    }
    // A notify-only Gate tells the person and routes the outcome that
    // opened it: nothing may answer it, so it offers nothing to answer.
    if gate.notify_only {
        if !gate.answers.is_empty() || gate.kind.is_some() || gate.person_only {
            return Err(FlowError::NotifyOnlyGateAnswers(entry.clone()));
        }
        let question = gate.question.trim();
        if question.is_empty() || gate.question.len() > MAX_QUESTION_BYTES {
            return Err(FlowError::IncompleteGate(entry.clone()));
        }
        return Ok(());
    }
    if let Some(kind) = gate.kind
        && !offers_kind_answers(kind, &gate.answers)
    {
        return Err(FlowError::GateKindAnswers {
            entry: entry.clone(),
            kind,
            answers: kind_answers(kind),
        });
    }
    let question = gate.question.trim();
    if question.is_empty() || gate.question.len() > MAX_QUESTION_BYTES || gate.answers.is_empty() {
        return Err(FlowError::IncompleteGate(entry.clone()));
    }

    let mut offered = BTreeSet::new();
    if let Some(twice) = gate.answers.iter().find(|answer| !offered.insert(*answer)) {
        return Err(FlowError::DuplicateAnswer {
            entry: entry.clone(),
            answer: twice.clone(),
        });
    }
    Ok(())
}
