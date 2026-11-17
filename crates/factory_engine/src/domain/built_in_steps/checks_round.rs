//! The checks round of the `write` and `fix` Steps: once the agent hands
//! back its work, the Step has the same agent session run the checks its
//! repository declares ([`crate::domain::checks`]), and sends a failure back
//! to it, until they pass or the repository's rounds run out.
//!
//! The agent runs the checks: the engine has no way to run a command where
//! the agent works (its machine's worktree or its managed sandbox), only to
//! prompt its session. Each round is recorded before its prompt is sent, so
//! a retried Step goes on with the round it was at, never starts one twice,
//! and asks under the prompt ids it recorded.

#[cfg(test)]
mod test;

use factory_records::domain::models::{
    CheckRound, CheckRoundOutcome, CheckRoundStart, Outcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use super::{BuiltInStepError, BuiltInSteps};
use crate::domain::checks::{CHECKS_FILE, RepositoryCheck};
use crate::domain::flow::StepName;
use crate::domain::models::{AgentTurn, StepInput, StepReport};
use crate::domain::ports::{PullRequests, StepAgent};
use crate::domain::pull_request_steps::PullRequestStepError;

/// What the agent is told for a round of checks.
const CHECKS_INSTRUCTIONS: &str = include_str!("checks.md");

/// What the agent is told when a round of checks failed.
const CHECKS_FIX_INSTRUCTIONS: &str = include_str!("checks_fix.md");

/// The outcome a write or fix Step ends with when the repository's checks
/// never passed.
pub(super) const RED: &str = "red";

/// Where a Step's checks are, from its rounds so far.
enum Next<'a> {
    /// No round started: the agent's first work is all there is.
    FirstRound,
    /// The round's checks run, or are about to.
    Run(&'a CheckRound),
    /// The round passed.
    Passed(&'a CheckRound),
    /// The round failed: its failure goes back to the agent, or no rounds
    /// are left.
    Failed(&'a CheckRound),
}

fn next(rounds: &[CheckRound]) -> Next<'_> {
    let Some(latest) = rounds.last() else {
        return Next::FirstRound;
    };
    match latest.end.as_ref().map(|end| end.outcome) {
        None => Next::Run(latest),
        Some(CheckRoundOutcome::Passed) => Next::Passed(latest),
        Some(CheckRoundOutcome::Failed) => Next::Failed(latest),
    }
}

fn records(error: impl std::error::Error + Send + Sync + 'static) -> BuiltInStepError {
    BuiltInStepError::PullRequest(PullRequestStepError::records(error))
}

impl<A: StepAgent, G: PullRequests, R: RunRepository> BuiltInSteps<A, G, R> {
    /// Have the agent do `step`'s work, then run the repository's checks on
    /// it in the same session, round after round. The Step ends with the
    /// agent's outcome when the work changed nothing or its branch declares
    /// no checks, `done` once they pass, and [`RED`] when the last round
    /// failed or the checks file does not read.
    pub(super) async fn work_and_check(
        &self,
        step: &StepName,
        instructions: &str,
        outcomes: Vec<Outcome>,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        let repo = self.pull_requests.records();
        let rounds = repo.check_rounds(input.key).await.map_err(records)?;

        // No round recorded: the agent's work is still to do, or a retry
        // finds its hand back in the session's history.
        let work = if rounds.is_empty() {
            let work = self.ask_agent(step, instructions, outcomes, input).await?;
            if work.outcome != Outcome::done() {
                return Ok(work);
            }
            Some(work)
        } else {
            None
        };

        loop {
            let rounds = repo.check_rounds(input.key).await.map_err(records)?;
            match next(&rounds) {
                Next::FirstRound => {
                    let Some(work) = &work else {
                        return Err(BuiltInStepError::Agent {
                            report: rootcause::report!("a recorded round of checks went missing")
                                .into_dynamic(),
                            retryable: false,
                        });
                    };
                    let started = self.start_round(input, 1, &work.output).await?;
                    if let Some(ended) = started {
                        return Ok(ended);
                    }
                }
                Next::Run(round) => {
                    self.run_round(step, input, round).await?;
                }
                Next::Passed(round) => {
                    return Ok(StepReport {
                        outcome: Outcome::done(),
                        output: format!(
                            "{}\n\nThe repository's checks passed in round {}: {}.",
                            round.work_output,
                            round.round,
                            round.checks.join(", ")
                        ),
                    });
                }
                Next::Failed(round) => {
                    let rounds_left = self.rounds_allowed(input).await?;
                    if round.round >= rounds_left {
                        return Ok(never_green(round));
                    }
                    let fixed = self.send_back(step, input, round).await?;
                    let started = self.start_round(input, round.round + 1, &fixed).await?;
                    if let Some(ended) = started {
                        return Ok(ended);
                    }
                }
            }
        }
    }

    /// How many rounds the branch's checks file allows now: none once the
    /// branch declares no checks that read, so a failed round ends the Step.
    async fn rounds_allowed(&self, input: StepInput<'_>) -> Result<u8, BuiltInStepError> {
        let checks = self.pull_requests.branch_checks(&input).await?;
        Ok(match checks {
            Some(Ok(checks)) => checks.rounds,
            Some(Err(_)) | None => 0,
        })
    }

    /// Record round `round` of the checks the branch runs now, on the work
    /// the agent handed back as `work_output`. The Step's end instead, when
    /// there is nothing to check or the checks file does not read.
    async fn start_round(
        &self,
        input: StepInput<'_>,
        round: u8,
        work_output: &str,
    ) -> Result<Option<StepReport>, BuiltInStepError> {
        let checks = match self.pull_requests.branch_checks(&input).await? {
            None => None,
            Some(Err(error)) => {
                return Ok(Some(StepReport {
                    outcome: red(),
                    output: format!(
                        "{work_output}\n\nThe repository's checks could not run: {error}"
                    ),
                }));
            }
            Some(Ok(checks)) => Some(checks.checks).filter(|checks| !checks.is_empty()),
        };
        let Some(checks) = checks else {
            return Ok(Some(StepReport {
                outcome: Outcome::done(),
                output: work_output.to_string(),
            }));
        };

        let start = CheckRoundStart {
            round,
            work_output: work_output.to_string(),
            checks: checks.iter().map(|check| check.name.clone()).collect(),
            check_action_id: Uuid::now_v7(),
        };
        self.pull_requests
            .records()
            .start_check_round(input.key, &start)
            .await
            .map_err(records)?;
        Ok(None)
    }

    /// Have the agent run `round`'s checks, and record how they went.
    async fn run_round(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        round: &CheckRound,
    ) -> Result<(), BuiltInStepError> {
        // The checks as the branch declares them now, of those the round
        // recorded: a retry asks for the same ones.
        let declared = match self.pull_requests.branch_checks(&input).await? {
            Some(Ok(checks)) => checks.checks,
            Some(Err(_)) | None => Vec::new(),
        };
        let checks: Vec<&RepositoryCheck> = declared
            .iter()
            .filter(|check| round.checks.contains(&check.name))
            .collect();
        let outcomes = vec![passed(), failed()];
        let answer = self
            .agent
            .follow_up(AgentTurn {
                key: input.key,
                owner: input.owner.clone(),
                action_id: round.check_action_id,
                prompt: checks_prompt(round.round, &checks),
                outcomes: outcomes.clone(),
            })
            .await
            .map_err(BuiltInStepError::agent)?;

        let outcome = if answer.outcome == passed() {
            CheckRoundOutcome::Passed
        } else if answer.outcome == failed() {
            CheckRoundOutcome::Failed
        } else {
            return Err(BuiltInStepError::UnexpectedOutcome {
                step: step.clone(),
                outcome: answer.outcome,
            });
        };
        self.pull_requests
            .records()
            .end_check_round(input.key, round.round, outcome, &answer.text)
            .await
            .map_err(records)
    }

    /// Send `round`'s failure back to the agent, and wait for its fix: what
    /// it handed back for it.
    async fn send_back(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        round: &CheckRound,
    ) -> Result<String, BuiltInStepError> {
        let key: StepKey = input.key;
        let action_id = self
            .pull_requests
            .records()
            .send_back_check_round(key, round.round, Uuid::now_v7())
            .await
            .map_err(records)?
            .ok_or_else(|| BuiltInStepError::Agent {
                report: rootcause::report!("round {} did not fail", round.round).into_dynamic(),
                retryable: false,
            })?;
        let report = round.end.as_ref().map_or("", |end| end.report.as_str());
        let answer = self
            .agent
            .follow_up(AgentTurn {
                key,
                owner: input.owner.clone(),
                action_id,
                prompt: format!("{CHECKS_FIX_INSTRUCTIONS}{report}\n"),
                outcomes: vec![Outcome::done()],
            })
            .await
            .map_err(BuiltInStepError::agent)?;
        if answer.outcome != Outcome::done() {
            return Err(BuiltInStepError::UnexpectedOutcome {
                step: step.clone(),
                outcome: answer.outcome,
            });
        }
        Ok(answer.text)
    }
}

/// How a Step whose last round of checks failed ends: [`RED`], saying so.
fn never_green(round: &CheckRound) -> StepReport {
    let report = round.end.as_ref().map_or("", |end| end.report.as_str());
    StepReport {
        outcome: red(),
        output: format!(
            "{}\n\nThe repository's checks still fail after {} rounds: {}.\n\n{report}",
            round.work_output,
            round.round,
            round.checks.join(", ")
        ),
    }
}

/// The prompt that asks the agent to run `checks` in round `round`.
fn checks_prompt(round: u8, checks: &[&RepositoryCheck]) -> String {
    let mut prompt = format!("{CHECKS_INSTRUCTIONS}Round {round}.\n\n");
    for check in checks {
        prompt.push_str(&format!("- `{}`: `{}`\n", check.name, check.run));
    }
    prompt.push_str(&format!("\nThey are declared in `{CHECKS_FILE}`.\n"));
    prompt
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).unwrap_or_else(|_| Outcome::failed())
}

fn red() -> Outcome {
    outcome(RED)
}

fn passed() -> Outcome {
    outcome(CheckRoundOutcome::Passed.as_str())
}

fn failed() -> Outcome {
    outcome(CheckRoundOutcome::Failed.as_str())
}
