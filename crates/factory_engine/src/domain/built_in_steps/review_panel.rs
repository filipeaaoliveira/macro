//! A reviewer's Step in a review round: what it is told, and reading what
//! it hands back. An answer that does not read is asked for once more in
//! the reviewer's session; one that still does not read is one Blocker on
//! the whole change, so a round never passes on it and the Run never fails
//! for it.
//!
//! A Blocker of the round before that the fix disputes is ruled on by the
//! reviewer whose Role found it (the round's first reviewer, for a finding
//! of no Role): it lists it again, and upholds it, or drops it. One it does
//! neither for stands as upheld, and blocks again, as does a carried
//! Blocker its brief had no room for. The same problem raised again at the
//! same lines by the reviewer that dropped it, in a later round of the
//! visit, is dismissed.

#[cfg(test)]
mod test;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::models::{Outcome, Role};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    DisputeRuling, Finding, FixStatus, ReviewFinding, ReviewRound, ReviewRoundRepository, Ruling,
};
use uuid::Uuid;

use super::checks_round::{lost, records};
use super::{BuiltInStepError, BuiltInSteps, REVIEW_FOCUS, REVIEW_PANEL_INSTRUCTIONS, outcomes};
use crate::domain::flow::StepName;
use crate::domain::models::{AgentTurn, StepInput, StepReport};
use crate::domain::ports::{MainWatches, PullRequests, StepAgent};
use crate::domain::review_rounds::{
    ReviewerAnswer, ReviewerAnswerError, breaks_brief, carried_blockers, dropped_in_visit,
    findings_block, fit_findings, places_meet,
};

/// The place of the prompt that asks a reviewer again among its Step's
/// later prompts: a reviewer's Step runs no checks, so it is the first.
const REASK_PLACE: u16 = 1;

/// The review panel's Role that reads the break records.
const TESTS_REVIEWER: &str = "tests-reviewer";

impl<A, G, R, M> BuiltInSteps<A, G, R, M>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository + ReviewRoundRepository,
    M: MainWatches,
{
    /// Have the reviewer of `input` review in `round`, record what it found
    /// on the round, and end with its summary and Blockers written out.
    pub(super) async fn review_in_round(
        &self,
        step: &StepName,
        round: &ReviewRound,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        let repo = self.pull_requests.records();
        let rounds = repo
            .review_rounds(input.key.run_id)
            .await
            .map_err(records)?;
        let carried = carried_blockers(&rounds, round);
        let brief = fit_findings(&carried);
        let first = input.key.seq == round.seq;
        // The carried Blockers this reviewer answers for: the ones its Role
        // found. Of those, it rules on each dispute whose Blocker and
        // reason the brief carries; the others stand as upheld.
        let mine: Vec<usize> = (1..)
            .zip(&carried)
            .filter(|(_, blocker)| rules_on(blocker, input.role, first))
            .map(|(number, _)| number)
            .collect();
        let rules: Vec<usize> = mine
            .iter()
            .copied()
            .filter(|number| {
                carried[number - 1].fix_status == Some(FixStatus::Disputed)
                    && !brief.left_out.contains(number)
                    && !brief.reasons_left_out.contains(number)
            })
            .collect();
        // What it dropped in an earlier round of the visit, never this one.
        let dropped = dropped_in_visit(&rounds, round, |seq| {
            seq < round.seq
                && input
                    .earlier
                    .iter()
                    .any(|step| step.seq == seq && step.role.as_ref() == input.role)
        });
        let mut instructions = panel_instructions(round, input.role, &carried, &rules, &dropped);
        // The tests reviewer reads what the factory saw of each new test's
        // break.
        if input
            .role
            .is_some_and(|role| role.as_str() == TESTS_REVIEWER)
        {
            let breaks = repo
                .runs_break_records(&[input.key.run_id])
                .await
                .map_err(records)?
                .remove(&input.key.run_id)
                .unwrap_or_default();
            instructions.push('\n');
            instructions.push_str(&breaks_brief(&breaks));
        }
        let report = self
            .ask_agent_in_repository(step, &instructions, review_outcomes(), input)
            .await?;
        // A reviewer that asks the Run's owner reviewed nothing yet: its
        // round waits for the answer, and its Role goes on with it in a
        // Step of its own, whose findings the round records.
        if report.outcome == Outcome::question() {
            return Ok(report);
        }

        let read =
            ReviewerAnswer::read_ruling(&report.outcome, &report.output, carried.len(), &rules);
        let (outcome, answer) = match read {
            Ok(answer) => (report.outcome, answer),
            Err(error) => {
                self.ask_again(round, input, carried.len(), &rules, &error)
                    .await?
            }
        };
        let answers_for = AnswersFor {
            carried: &carried,
            mine: &mine,
            left_out: &brief.left_out,
            dropped: &dropped,
        };
        let (outcome, answer, rulings) = rule(outcome, answer, &answers_for);

        let recorded = repo
            .record_answer(input.key, round.seq, &answer.findings, &rulings)
            .await
            .map_err(records)?;
        if !recorded {
            return Err(lost(
                "the reviewer's step ended before its findings were recorded",
            ));
        }
        Ok(StepReport {
            outcome,
            output: answer.render(),
        })
    }

    /// Ask the reviewer of `input` once more, in its session, for an answer
    /// that reads, saying why the first did not; one that still does not
    /// read is one Blocker on the whole change. The prompt is kept before it
    /// is sent, so a retry asks in the same one.
    async fn ask_again(
        &self,
        round: &ReviewRound,
        input: StepInput<'_>,
        carried: usize,
        rules: &[usize],
        error: &ReviewerAnswerError,
    ) -> Result<(Outcome, ReviewerAnswer), BuiltInStepError> {
        let repo = self.pull_requests.records();
        let action_id = repo
            .reask_action(input.key, round.seq, Uuid::now_v7())
            .await
            .map_err(records)?;
        let role = input
            .role
            .cloned()
            .ok_or_else(|| lost("a reviewer's step names no role"))?;
        let turn = AgentTurn {
            key: input.key,
            owner: input.owner.clone(),
            role,
            action_id,
            place: REASK_PLACE,
            prompt: reask_prompt(error),
            outcomes: review_outcomes(),
            answer_by: self.rounds_end_by(input).await?,
        };
        let again = self
            .agent
            .follow_up(turn)
            .await
            .map_err(BuiltInStepError::agent)?;

        let changes = Outcome::try_from("changes".to_string()).map_err(|_| lost("no outcome"))?;
        let read = if review_outcomes().contains(&again.outcome) {
            ReviewerAnswer::read_ruling(&again.outcome, &again.text, carried, rules)
        } else {
            Err(ReviewerAnswerError::OutcomeAndBlockers)
        };
        match read {
            Ok(answer) => Ok((again.outcome, answer)),
            Err(error) => Ok((changes, ReviewerAnswer::unreadable(&again.text, &error))),
        }
    }
}

/// Whether the reviewer of `role` rules on the dispute of `blocker`: it
/// found it, or, for a finding of no Role, it is the round's `first`
/// reviewer. A panel names each Role once, and a Run keeps its panel, so
/// one reviewer of each round rules on each dispute.
fn rules_on(blocker: &ReviewFinding, role: Option<&Role>, first: bool) -> bool {
    match &blocker.role {
        Some(found_by) => role == Some(found_by),
        None => first,
    }
}

/// The carried Blockers a reviewer answers for, and what it dropped.
struct AnswersFor<'a> {
    /// The Blockers of the round before, numbered from 1.
    carried: &'a [&'a ReviewFinding],
    /// The numbers of those its Role found: it answers for them.
    mine: &'a [usize],
    /// The numbers of those its brief left out for lack of room.
    left_out: &'a [usize],
    /// The Blockers it dropped in an earlier round of the visit.
    dropped: &'a [&'a ReviewFinding],
}

/// The answer as the round records it, with its rulings on the disputes
/// among the carried Blockers it answers for. A dispute it dropped is
/// dropped; any other is upheld. A Blocker that stands and that it did not
/// list again is added to its Blockers as the reviewer of the round before
/// wrote it, blocking wherever it is: an upheld dispute, and any Blocker its
/// brief left out, which it never saw. So neither a dispute nor a lack of
/// room passes a round. A new Blocker that repeats one it dropped in an
/// earlier round of the visit, at the same lines with the same problem, is
/// marked so the close dismisses it. An answer that passes but gains a
/// Blocker asks for changes.
fn rule(
    outcome: Outcome,
    mut answer: ReviewerAnswer,
    answers_for: &AnswersFor<'_>,
) -> (Outcome, ReviewerAnswer, Vec<Ruling>) {
    let mut rulings = Vec::new();
    let mut stands = Vec::new();
    for &number in answers_for.mine {
        let Some(blocker) = answers_for.carried.get(number - 1) else {
            continue;
        };
        let disputed = blocker.fix_status == Some(FixStatus::Disputed);
        let dropped = disputed && answer.dropped.contains(&number);
        let unseen = answers_for.left_out.contains(&number);
        if disputed {
            rulings.push(Ruling {
                seq: blocker.seq,
                place: blocker.place,
                ruling: if dropped {
                    DisputeRuling::Dropped
                } else {
                    DisputeRuling::Upheld
                },
            });
        }
        if !dropped && (disputed || unseen) && !answer.listed_again.contains(&number) {
            stands.push(Finding {
                path: blocker.path.clone(),
                lines: blocker.lines,
                problem: blocker.problem.clone(),
                why: blocker.why.clone(),
                blocks_anywhere: true,
                repeats_dropped: false,
            });
        }
    }

    for finding in answer
        .findings
        .iter_mut()
        .filter(|finding| !finding.blocks_anywhere)
    {
        finding.repeats_dropped = answers_for
            .dropped
            .iter()
            .any(|blocker| repeats(blocker, finding));
    }

    let outcome = if stands.is_empty() {
        outcome
    } else {
        Outcome::try_from("changes".to_string()).unwrap_or(outcome)
    };
    answer.findings.extend(stands);
    (outcome, answer, rulings)
}

/// Whether `finding` repeats the dropped Blocker `dropped`: the same file,
/// at lines that meet, with the same problem once case, punctuation and
/// spacing are set aside. A finding of no lines, or a Blocker of none,
/// repeats nothing: a whole file holds more than one problem.
fn repeats(dropped: &ReviewFinding, finding: &Finding) -> bool {
    dropped.lines.is_some()
        && finding.lines.is_some()
        && places_meet(&dropped.path, dropped.lines, &finding.path, finding.lines)
        && same_text(&dropped.problem) == same_text(&finding.problem)
}

/// `text` lowercased, with only its letters and digits, one space between
/// words.
fn same_text(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The outcomes a reviewer ends with.
pub(crate) fn review_outcomes() -> Vec<Outcome> {
    outcomes(&["pass", "changes"])
}

/// What a reviewer whose answer did not read is asked.
fn reask_prompt(error: &ReviewerAnswerError) -> String {
    format!(
        "Your review did not read: {error}. Hand it back again with \
         `macro.factory.handBack({{ outcome, text }})`, `pass` or `changes`, its text ending \
         with one fenced `json` block, `{{\"blockers\": [...]}}`, as your first prompt says: \
         each blocker with its `file`, optional `startLine` and `endLine`, `problem` and `why`, \
         and `previous` when it is a blocker of the round before found again."
    )
}

/// What a reviewer of `round` filling `role` is told: what its Role looks
/// at, the commit it reviews, the diff under review (the Run's whole change
/// or the fix diff), and the Blockers of the round before, `carried`, to
/// check again.
pub(super) fn panel_instructions(
    round: &ReviewRound,
    role: Option<&Role>,
    carried: &[&ReviewFinding],
    rules: &[usize],
    dropped: &[&ReviewFinding],
) -> String {
    let focus = REVIEW_FOCUS
        .iter()
        .find(|(name, _)| role.is_some_and(|role| role.as_str() == *name))
        .map_or("", |(_, focus)| focus.trim());
    let head = round.head_sha.as_deref().unwrap_or_default();
    let diff_base = round.diff_base.as_deref().unwrap_or_default();
    let (base, scope) = if round.fix_diff {
        (
            diff_base.to_string(),
            format!(
                "the changes the fix made since review round {} reviewed `{diff_base}`",
                round.round.saturating_sub(1),
            ),
        )
    } else {
        (
            format!("origin/{diff_base}"),
            "the run's whole change against its base".to_string(),
        )
    };
    let mut instructions = REVIEW_PANEL_INSTRUCTIONS
        .replace("{focus}", focus)
        .replace("{head}", head)
        .replace("{base}", &base)
        .replace("{scope}", &scope);
    if !carried.is_empty() {
        instructions.push_str(&format!(
            "\n## Blockers of the round before\n\nThe round before found these blockers, and \
             the fix was to address each. Check every one at `{head}`, wherever it is now, \
             not only in the diff under review. List each one still there as a blocker, with \
             `\"previous\"` set to its number below: it then blocks wherever it is.\n\n{}",
            findings_block(carried)
        ));
    }
    if !rules.is_empty() {
        let numbers: Vec<String> = rules.iter().map(ToString::to_string).collect();
        instructions.push_str(&format!(
            "\n## Disputed blockers\n\nThe fix disputes the blockers numbered {} above, \
             which you found, with the reasons the block gives. Rule on each one: list it \
             again as a blocker, with `\"previous\"` set to its number, to uphold it; or add \
             its number to `\"dropped\"` in your json block (`{{\"blockers\": [], \
             \"dropped\": [2]}}`) to drop it, when the fixer's reason holds. A dropped \
             blocker blocks nothing. A dispute you neither list again nor drop is upheld, \
             and blocks again.\n",
            numbers.join(", ")
        ));
    } else if carried.iter().any(|blocker| blocker.dispute.is_some()) {
        instructions.push_str(
            "\nThe reviewer that found a disputed blocker above rules on its dispute: list \
             one again only when your own focus finds it still there.\n",
        );
    }
    if !dropped.is_empty() {
        instructions.push_str(&format!(
            "\n## Blockers you dropped\n\nYou dropped these disputed blockers earlier in this \
             review. The same problem raised again at the same lines blocks nothing, so leave \
             it out. A new problem at the same place is a blocker like any other: raise it.\n\n{}",
            findings_block(dropped)
        ));
    }
    instructions
}
