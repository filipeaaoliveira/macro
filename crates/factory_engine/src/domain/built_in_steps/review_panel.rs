//! A reviewer's Step in a review round: what it is told, and reading what
//! it hands back. An answer that does not read is asked for once more in
//! the reviewer's session; one that still does not read is one Blocker on
//! the whole change, so a round never passes on it and the Run never fails
//! for it.

#[cfg(test)]
mod test;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::models::{Outcome, Role};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{ReviewFinding, ReviewRound, ReviewRoundRepository};
use uuid::Uuid;

use super::checks_round::{lost, records};
use super::{BuiltInStepError, BuiltInSteps, REVIEW_FOCUS, REVIEW_PANEL_INSTRUCTIONS, outcomes};
use crate::domain::flow::StepName;
use crate::domain::models::{AgentTurn, StepInput, StepReport};
use crate::domain::ports::{MainWatches, PullRequests, StepAgent};
use crate::domain::review_rounds::{
    ReviewerAnswer, ReviewerAnswerError, carried_blockers, findings_block,
};

/// The place of the prompt that asks a reviewer again among its Step's
/// later prompts: a reviewer's Step runs no checks, so it is the first.
const REASK_PLACE: u16 = 1;

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
        let instructions = panel_instructions(round, input.role, &carried);
        let report = self
            .ask_agent_in_repository(step, &instructions, review_outcomes(), input)
            .await?;
        // A reviewer that asks the Run's owner reviewed nothing yet: its
        // round waits for the answer, and its Role goes on with it in a
        // Step of its own, whose findings the round records.
        if report.outcome == Outcome::question() {
            return Ok(report);
        }

        let (outcome, answer) =
            match ReviewerAnswer::read(&report.outcome, &report.output, carried.len()) {
                Ok(answer) => (report.outcome, answer),
                Err(error) => self.ask_again(round, input, carried.len(), &error).await?,
            };

        let recorded = repo
            .record_findings(input.key, round.seq, &answer.findings)
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
            ReviewerAnswer::read(&again.outcome, &again.text, carried)
        } else {
            Err(ReviewerAnswerError::OutcomeAndBlockers)
        };
        match read {
            Ok(answer) => Ok((again.outcome, answer)),
            Err(error) => Ok((changes, ReviewerAnswer::unreadable(&again.text, &error))),
        }
    }
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
    instructions
}
