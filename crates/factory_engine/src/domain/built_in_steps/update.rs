//! The `update` Step: a Run whose branch lacks a commit of its base, found
//! at its merge, merges the base into the branch, then runs the
//! repository's checks on it again, as `write` and `fix` do
//! (`checks_round`).
//!
//! The base is merged in, never rebased onto: the branch keeps every commit
//! it had, so nothing anyone pushed to it is lost, no push is forced, and
//! the commits earlier review rounds and checks recorded stay on it. GitHub
//! merges it ("Update branch", only while the branch's head is the one
//! read); when it cannot, as for a conflict, the Run's writer resolves it
//! in its session, or hands it to a person (`conflict`, which the `code`
//! Flow asks about at a Gate).
//!
//! The Step ends:
//! - `done` when the branch's head is still the one the merge GitHub made
//!   on the factory's asking made, and its checks passed: only the base
//!   came in, so the Run goes on to CI, and the merge Gate asks again;
//! - `resolved` when anything else is on it now, as an agent's resolution
//!   of a conflict, a person's pushed before the conflict Gate's `retry`
//!   (the branch then holds its base already), or a fix a checks round
//!   asked for: the change itself changed, so its review and acceptance
//!   run again;
//! - `conflict` when the agent could not resolve the conflict, or what it
//!   pushed does not hold the base and every commit the branch had;
//! - `red` when the checks never passed, and `merged` or `closed` when the
//!   pull request was merged or closed meanwhile.

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::{BranchUpdate, MergeQueueRepository};
use factory_records::domain::models::Outcome;
use factory_records::domain::ports::RunRepository;

use super::{BuiltInStepError, BuiltInSteps, outcomes};
use crate::domain::flow::StepName;
use crate::domain::models::{StepInput, StepReport, run_branch};
use crate::domain::ports::{MainWatches, PullRequests, StepAgent};
use crate::domain::pull_request_steps::Update;

/// What the agent is told once GitHub merged the base into the branch.
const UPDATE_INSTRUCTIONS: &str = include_str!("update.md");

/// What the agent is told when GitHub could not merge them.
const RESOLVE_INSTRUCTIONS: &str = include_str!("resolve.md");

/// The outcome of an update that changed more than the base brought in.
const RESOLVED: &str = "resolved";

/// The outcome of an update no agent could resolve.
const CONFLICT: &str = "conflict";

impl<A, G, R, M> BuiltInSteps<A, G, R, M>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository,
    M: MainWatches,
{
    /// Merge the Run's base into its branch and check it again.
    pub(super) async fn update_branch(
        &self,
        step: &StepName,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        if input.repository.is_none() {
            return Err(BuiltInStepError::NoRepository(step.clone()));
        }
        match self.pull_requests.update_branch(&input).await? {
            Update::Ended(report) => Ok(report),
            Update::Merged(update) => {
                let head = update.updated_head.clone().unwrap_or_default();
                // Only a merge GitHub made on the factory's asking moved the
                // branch from where the update started; a branch that held
                // its base already holds someone else's commits (a person's
                // or an agent's resolution after the conflict Gate's
                // `retry`), which no review has seen.
                let factory_merge = update.from_head != head;
                let instructions = UPDATE_INSTRUCTIONS
                    .replace("{base_sha}", &update.base_sha)
                    .replace("{head}", &head);
                let checked = self
                    .work_and_check(step, &instructions, vec![Outcome::done()], input)
                    .await?;
                if checked.outcome != Outcome::done() {
                    return Ok(checked);
                }
                if !factory_merge {
                    return Ok(StepReport {
                        outcome: outcome(RESOLVED),
                        output: format!(
                            "The branch already held the base at {head}, through commits the \
                             factory did not make, so its review runs again.\n\n{}",
                            checked.output
                        ),
                    });
                }
                let now = self.pull_requests.branch_head(&input).await?;
                if now.as_deref() == Some(head.as_str()) {
                    return Ok(checked);
                }
                Ok(StepReport {
                    outcome: outcome(RESOLVED),
                    output: format!(
                        "The base was merged into the branch at {head}, and the branch changed \
                         since, so its review runs again.\n\n{}",
                        checked.output
                    ),
                })
            }
            Update::Conflict(update) => {
                let instructions = RESOLVE_INSTRUCTIONS
                    .replace("{base_sha}", &update.base_sha)
                    .replace("{branch}", &run_branch(input.key.run_id));
                let resolved = self
                    .work_and_check(step, &instructions, outcomes(&["done", CONFLICT]), input)
                    .await?;
                if resolved.outcome != Outcome::done() {
                    return Ok(resolved);
                }
                self.check_resolution(input, &update, resolved).await
            }
        }
    }

    /// What an agent that resolved a conflict pushed: `resolved` when the
    /// branch holds both the base it merged and every commit it had, so the
    /// change is reviewed again; otherwise `conflict`, for a person.
    async fn check_resolution(
        &self,
        input: StepInput<'_>,
        update: &BranchUpdate,
        resolved: StepReport,
    ) -> Result<StepReport, BuiltInStepError> {
        let head = self.pull_requests.branch_head(&input).await?;
        let holds = match &head {
            Some(head) => {
                self.pull_requests
                    .holds(&input, &update.base_sha, head)
                    .await?
                    && self
                        .pull_requests
                        .holds(&input, &update.from_head, head)
                        .await?
            }
            None => false,
        };
        if holds {
            return Ok(StepReport {
                outcome: outcome(RESOLVED),
                ..resolved
            });
        }
        Ok(StepReport {
            outcome: outcome(CONFLICT),
            output: format!(
                "The agent said it resolved the conflict, but the branch's head does not hold \
                 both the base's {} and the branch's {}: a person resolves it.\n\n{}",
                update.base_sha, update.from_head, resolved.output
            ),
        })
    }
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).unwrap_or_else(|_| Outcome::failed())
}
