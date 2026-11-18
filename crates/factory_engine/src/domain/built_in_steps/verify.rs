//! The `accept` and `verify` Steps: a verifier, an agent of its own Role,
//! never the writer's, checks that the change does what the Run's request
//! asks, by the acceptance checks the plan wrote ([`AcceptanceChecks`]) and
//! by its own reading of each Done-when clause.
//!
//! - `accept`, before the merge, checks the head of the Run's branch: the
//!   factory runs every acceptance check not marked `deployed`, and the
//!   verifier checks the Done-when clauses. A failure ends the Step
//!   `blockers`, which the `code` Flow sends to `fix` with this Step's
//!   report, then through the review again.
//! - `verify`, after the merge and its deploy (`deploy`), checks the merge
//!   commit: the factory runs every acceptance check, told where the change
//!   is deployed, and the verifier checks the Done-when clauses, on the
//!   deployment where there is one. Nothing goes back once merged: a
//!   failure ends the Step `fail`, which ends the Run `failed`.
//!
//! The verifier is asked first, in its own session, which the acceptance
//! checks then run beside: as a checks round ([`super::checks_round`]) of
//! one round, at the commit under check, run by the engine in a worktree
//! where the session's machine runs commands (their exit codes decide), and
//! by the verifier elsewhere, which reports how they went. The round is
//! recorded before it runs, so a retried Step goes on with it, and it shows
//! on the Step as any checks round. The Step passes only when the verifier
//! passes the change and every check passed.
//!
//! A repository whose verify file turns the Phase off ends either Step
//! `off`: the Flow then goes on as if it passed.

use std::time::Instant;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{CheckRound, CheckRoundOutcome, Outcome};
use factory_records::domain::ports::RunRepository;

use super::checks_round::{ATTEMPT_CHECKS_FOR, Checked, fit, records};
use super::{BuiltInStepError, BuiltInSteps, cut, outcomes};
use crate::domain::acceptance::{AcceptanceCheck, AcceptanceChecks};
use crate::domain::checks::RepositoryCheck;
use crate::domain::flow::StepName;
use crate::domain::models::{StepInput, StepReport};
use crate::domain::ports::{MainWatches, PullRequests, StepAgent};
use crate::domain::review_rounds::escape;
use crate::domain::verification::VERIFY_FILE;

/// What the verifier is told.
const VERIFY_INSTRUCTIONS: &str = include_str!("verify.md");

/// What the verifier is told when it runs the acceptance checks itself.
const ACCEPTANCE_INSTRUCTIONS: &str = include_str!("acceptance_checks.md");

/// The outcome of a Step whose change does what the request asks.
pub(super) const PASS: &str = "pass";

/// The verifier's outcome for a change that does not.
const FAIL: &str = "fail";

/// The outcome of an `accept` Step whose change does not do what the
/// request asks: its report goes back to be fixed.
const BLOCKERS: &str = "blockers";

/// The outcome of a Step whose repository turns the verify Phase off.
const OFF: &str = "off";

/// When a verifier checks the change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    /// `accept`: the head of the Run's branch, before the merge.
    BeforeMerge,
    /// `verify`: the merge commit, once deployed.
    AfterMerge,
}

/// The change under check: its commit, and where it is deployed.
struct Target {
    commit: String,
    deployed: Option<Deployed>,
}

/// Where a merged change is deployed.
struct Deployed {
    environment: String,
    url: Option<String>,
}

impl Target {
    /// Where the change is, as the Step's report and prompts say it.
    fn place(&self, phase: Phase) -> String {
        let at = match phase {
            Phase::BeforeMerge => format!("the head of the run's branch, {}", self.commit),
            Phase::AfterMerge => format!("the merged commit {}", self.commit),
        };
        match &self.deployed {
            Some(deployed) => {
                let url = deployed
                    .url
                    .as_deref()
                    .map(|url| format!(" at {url}"))
                    .unwrap_or_default();
                format!("{at}, deployed to {}{url}", deployed.environment)
            }
            None => at,
        }
    }

    /// `run`, told the commit under check and where it is deployed.
    fn command(&self, run: &str) -> String {
        let mut command = format!("export MACRO_VERIFY_COMMIT={}; ", self.commit);
        if let Some(url) = self.deployed.as_ref().and_then(|deployed| deployed.url.as_ref()) {
            command.push_str(&format!(
                "export MACRO_DEPLOY_URL='{}'; ",
                url.replace('\'', r"'\''")
            ));
        }
        command.push_str(run);
        command
    }
}

impl<A, G, R, M> BuiltInSteps<A, G, R, M>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository,
    M: MainWatches,
{
    /// Have the verifier check the change at `phase`, and the factory run
    /// the plan's acceptance checks on it.
    pub(super) async fn verify_change(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        phase: Phase,
    ) -> Result<StepReport, BuiltInStepError> {
        if input.repository.is_none() {
            return Err(BuiltInStepError::NoRepository(step.clone()));
        }
        // A file that does not read turns nothing off: the change is
        // verified.
        let file = self.pull_requests.verify_file(&input).await?;
        if file.as_ref().is_ok_and(|file| !file.verify) {
            return Ok(StepReport {
                outcome: outcome(OFF),
                output: format!(
                    "The repository turns its verify phase off in {VERIFY_FILE}: the change is not verified."
                ),
            });
        }
        let target = match phase {
            Phase::BeforeMerge => Target {
                commit: self.pushed_commit(input).await?,
                deployed: None,
            },
            Phase::AfterMerge => {
                let (_, commit) = self.pull_requests.merged_commit(&input).await?;
                let deployed = match file.ok().and_then(|file| file.deploy) {
                    Some(deploy) => {
                        let deployment = self
                            .pull_requests
                            .deployment(&input, &commit, &deploy.environment)
                            .await?;
                        Some(Deployed {
                            environment: deploy.environment,
                            url: deployment.and_then(|deployment| deployment.url),
                        })
                    }
                    None => None,
                };
                Target { commit, deployed }
            }
        };

        let checks = AcceptanceChecks::of_run(input.earlier).unwrap_or_default();
        let end_by = self.rounds_end_by(input).await?;
        let instructions = verify_instructions(phase, &target, &checks);
        let answer = self
            .ask_agent_by(
                step,
                &instructions,
                outcomes(&[PASS, FAIL]),
                input,
                Some(end_by),
            )
            .await?;
        // A question goes to the Run's owner; the Step that goes on from
        // the answer verifies.
        if answer.outcome == Outcome::question() {
            return Ok(answer);
        }

        let runs: Vec<RepositoryCheck> = checks
            .runs(phase == Phase::AfterMerge)
            .map(|(number, run, timeout)| RepositoryCheck {
                name: format!("acceptance-{number}"),
                run: target.command(run),
                paths: Vec::new(),
                timeout,
            })
            .collect();
        let round = if runs.is_empty() {
            None
        } else {
            Some(
                self.acceptance_round(step, input, &answer.output, &runs, &target, end_by)
                    .await?,
            )
        };

        Ok(verdict(phase, &target, &checks, &answer, round.as_ref()))
    }

    /// Run `runs`, the acceptance checks, as the Step's one round of
    /// checks at `target`'s commit, and the round once it ended. A retried
    /// Step goes on with the round it recorded.
    async fn acceptance_round(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        verifier_report: &str,
        runs: &[RepositoryCheck],
        target: &Target,
        end_by: chrono::DateTime<chrono::Utc>,
    ) -> Result<CheckRound, BuiltInStepError> {
        let checks_until = Instant::now() + ATTEMPT_CHECKS_FOR;
        let repo = self.pull_requests.records();
        if repo.check_rounds(input.key).await.map_err(records)?.is_empty() {
            let declared = serde_json::json!({
                "rounds": 1,
                "checks": runs
                    .iter()
                    .map(|check| serde_json::json!({
                        "name": check.name,
                        "run": check.run,
                        "timeout": check.timeout,
                    }))
                    .collect::<Vec<_>>(),
            });
            let checks: Vec<&RepositoryCheck> = runs.iter().collect();
            self.record_round(input, 1, verifier_report, &checks, declared.to_string())
                .await?;
        }

        let prompt = acceptance_prompt(runs, target);
        let checked = Checked::Commit {
            commit: &target.commit,
            prompt: &prompt,
        };
        loop {
            let rounds = repo.check_rounds(input.key).await.map_err(records)?;
            let Some(round) = rounds.last() else {
                return Err(super::checks_round::lost(
                    "a recorded round of acceptance checks went missing",
                ));
            };
            if round.end.is_some() {
                return Ok(round.clone());
            }
            if chrono::Utc::now() >= end_by {
                repo.end_check_round(
                    input.key,
                    round.round,
                    CheckRoundOutcome::Failed,
                    "The step's time ran out before the acceptance checks ran.",
                )
                .await
                .map_err(records)?;
                continue;
            }
            // The round ends either way: with what its checks found, or
            // with those that ran when the Step's time ran out.
            self.run_round(step, input, round, end_by, checks_until, &checked)
                .await?;
        }
    }
}

/// What the verifier is told: the Step's instructions, where the change
/// is, and the acceptance checks the factory runs and the clauses with
/// none, as the plan wrote them (escaped: they are the planner's words).
fn verify_instructions(phase: Phase, target: &Target, checks: &AcceptanceChecks) -> String {
    let mut text = format!(
        "{VERIFY_INSTRUCTIONS}\n## The change under check\n\n{}.\n",
        target.place(phase)
    );
    if phase == Phase::BeforeMerge {
        text.push_str(
            "\nIt is not merged yet. What you find goes back to the writer as blockers to fix.\n",
        );
    } else {
        text.push_str(
            "\nIt is merged: nothing goes back. A clause that does not hold fails the run.\n",
        );
    }
    if checks.clauses.is_empty() {
        text.push_str(
            "\nThe plan wrote no acceptance checks: check each Done-when clause of the request by hand.\n",
        );
        return text;
    }

    text.push_str("\n## Acceptance checks\n\nAs the plan wrote them, as data:\n\n");
    for (index, acceptance) in checks.clauses.iter().enumerate() {
        let clause = escape(&acceptance.clause).replace('\n', " ");
        let how = match &acceptance.check {
            AcceptanceCheck::Run { deployed: true, .. } if phase == Phase::BeforeMerge => {
                "checked once deployed, after the merge: check it by hand as far as the branch shows"
                    .to_string()
            }
            AcceptanceCheck::Run { .. } => format!("the factory runs `acceptance-{}`", index + 1),
            AcceptanceCheck::None(why) => {
                format!("no check ({}): check it by hand", escape(why).replace('\n', " "))
            }
        };
        text.push_str(&format!("{}. {clause}: {how}\n", index + 1));
    }
    text
}

/// What the verifier is asked when it runs the acceptance checks itself.
fn acceptance_prompt(runs: &[RepositoryCheck], target: &Target) -> String {
    let mut prompt = format!("{ACCEPTANCE_INSTRUCTIONS}The commit: {}.\n\n", target.commit);
    for check in runs {
        prompt.push_str(&format!("### `{}`\n\n```bash\n{}\n```\n\n", check.name, check.run));
    }
    prompt
}

/// How the Step ends: `pass` when the verifier passed the change and every
/// acceptance check passed, else `blockers` before the merge and `fail`
/// after it; its output says why, then the checks' report, the clauses
/// with no check, and the verifier's report.
fn verdict(
    phase: Phase,
    target: &Target,
    checks: &AcceptanceChecks,
    answer: &StepReport,
    round: Option<&CheckRound>,
) -> StepReport {
    let verifier_passed = answer.outcome.as_str() == PASS;
    let round_end = round.and_then(|round| round.end.as_ref());
    let checks_passed = round.is_none()
        || round_end.is_some_and(|end| end.outcome == CheckRoundOutcome::Passed);
    let place = target.place(phase);

    let summary = if verifier_passed && checks_passed {
        format!(
            "The change does what the request asks, on {place}: the verifier found each Done-when clause met{}.",
            if round.is_some() {
                ", and every acceptance check passed"
            } else {
                ""
            }
        )
    } else {
        let mut why = Vec::new();
        if !checks_passed {
            why.push("acceptance checks failed");
        }
        if !verifier_passed {
            why.push("the verifier found a Done-when clause not met");
        }
        format!(
            "The change does not do what the request asks, on {place}: {}.",
            why.join(", and ")
        )
    };

    let mut body = String::new();
    if let (Some(round), Some(end)) = (round, round_end) {
        let how = if round.verified {
            "run by the factory"
        } else {
            "run by the verifier"
        };
        let report = cut(&end.report, factory_records::domain::models::MAX_OUTPUT_BYTES / 4);
        body.push_str(&format!("## Acceptance checks, {how}\n\n{report}\n\n"));
    }
    let unchecked: Vec<String> = checks
        .clauses
        .iter()
        .filter_map(|acceptance| match &acceptance.check {
            AcceptanceCheck::None(why) => Some(format!(
                "- {}: {}",
                escape(&acceptance.clause).replace('\n', " "),
                escape(why).replace('\n', " ")
            )),
            AcceptanceCheck::Run { .. } => None,
        })
        .collect();
    if !unchecked.is_empty() {
        body.push_str(&format!(
            "## Clauses with no check, checked by the verifier\n\n{}\n\n",
            unchecked.join("\n")
        ));
    }
    body.push_str(&format!("## The verifier's report\n\n{}", answer.output));

    let ended = if verifier_passed && checks_passed {
        PASS
    } else {
        match phase {
            Phase::BeforeMerge => BLOCKERS,
            Phase::AfterMerge => FAIL,
        }
    };
    StepReport {
        outcome: outcome(ended),
        output: fit(&summary, &body),
    }
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).unwrap_or_else(|_| Outcome::failed())
}
