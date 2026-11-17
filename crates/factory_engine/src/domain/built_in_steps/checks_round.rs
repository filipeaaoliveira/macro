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
//!
//! The checks file is read from the Run's base branch, which the agent does
//! not push to, once: the first round keeps it, with the checks it runs, and
//! every later round goes by that record. A later round runs the checks the
//! first one ran, and any others the branch's changes now reach; a check
//! that a later round reached only through the changes is dropped again
//! once they no longer reach it, as the first round's checks are the ones
//! the Step answers for.
//!
//! The agent's work and all rounds run in the Step's one activity, whose
//! retries end two hours after it was first scheduled. Waiting for the work
//! and the rounds stops short of that ([`ROUNDS_END_BY`] after the Step
//! started), and the Step ends [`RED`], saying why, rather than fail; a
//! round still open then ends failed. Each wait is also bounded by its
//! attempt (`SESSION_DEADLINE` from when the wait began): a later prompt's
//! wait that ends there is retried and goes on with the same turn.

#[cfg(test)]
mod test;

use std::time::Duration;

use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    CheckRound, CheckRoundOutcome, CheckRoundStart, HandedBack, MAX_OUTPUT_BYTES, Outcome, Role,
};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use super::{BuiltInStepError, BuiltInSteps, agent_role, cut};
use crate::domain::checks::{CHECKS_FILE, RepositoryCheck, RepositoryChecks};
use crate::domain::flow::StepName;
use crate::domain::models::{AgentTurn, StepInput, StepReport};
use crate::domain::ports::{MainWatches, PullRequests, StepAgent};
use crate::domain::pull_request_steps::PullRequestStepError;

/// What the agent is told for a round of checks.
const CHECKS_INSTRUCTIONS: &str = include_str!("checks.md");

/// What the agent is told when a round of checks failed.
const CHECKS_FIX_INSTRUCTIONS: &str = include_str!("checks_fix.md");

/// The outcome a write or fix Step ends with when the repository's checks
/// never passed.
pub(super) const RED: &str = "red";

/// How long after a Step started its rounds may go on: short of the two
/// hours its activity's retries have (the workflow's `STEP_TOTAL`), so the
/// Step ends with what the rounds found.
pub(crate) const ROUNDS_END_BY: Duration = Duration::from_secs(105 * 60);

/// What a Step whose agent was still at work when its time ran out says.
const WORK_OUT_OF_TIME: &str = "The agent's work did not end before the step's time ran out, so the repository's checks did not run.";

/// What a round the agent could not run for lack of quota reports.
const QUOTA_RAN_OUT: &str =
    "The agent's quota ran out during the checks, so it could not run them.";

/// What ends an output cut to fit what a Step keeps.
const CUT_MARK: &str = "\n\n(The rest is left out: a step's output holds 64 KiB.)";

/// Where a Step's checks are, from its rounds so far.
enum Next<'a> {
    /// No round started: the agent's first work is all there is.
    FirstRound,
    /// The round's checks run, or are about to.
    Run(&'a CheckRound),
    /// The latest round passed.
    Passed,
    /// The round failed: its failure goes back to the agent, or no rounds
    /// are left.
    Failed(&'a CheckRound),
    /// The agent's quota ran out on the round: nothing follows it.
    QuotaRanOut,
}

fn next(rounds: &[CheckRound]) -> Next<'_> {
    let Some(latest) = rounds.last() else {
        return Next::FirstRound;
    };
    match latest.end.as_ref().map(|end| end.outcome) {
        None => Next::Run(latest),
        Some(CheckRoundOutcome::Passed) => Next::Passed,
        Some(CheckRoundOutcome::Failed) if latest.quota_ran_out => Next::QuotaRanOut,
        Some(CheckRoundOutcome::Failed) => Next::Failed(latest),
    }
}

/// The place of round `round`'s checks prompt among the Step's later
/// prompts; the fix of its failure comes just after it.
fn checks_place(round: u8) -> u16 {
    u16::from(round) * 2
}

/// The Role whose Agent's session a later prompt of `step` goes to.
fn turn_role(step: &StepName, input: StepInput<'_>) -> Result<Role, BuiltInStepError> {
    agent_role(step, input.role).ok_or_else(|| BuiltInStepError::NotAnAgentStep(step.clone()))
}

fn records(error: impl std::error::Error + Send + Sync + 'static) -> BuiltInStepError {
    BuiltInStepError::PullRequest(PullRequestStepError::records(error))
}

fn lost(why: &str) -> BuiltInStepError {
    BuiltInStepError::Agent {
        report: rootcause::report!("{why}").into_dynamic(),
        retryable: false,
        out_of_quota: false,
    }
}

impl<A, G, R, M> BuiltInSteps<A, G, R, M>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository,
    M: MainWatches,
{
    /// Have the agent do `step`'s work, then run the repository's checks on
    /// it in the same session, round after round. The Step ends with the
    /// agent's outcome when the work changed nothing or the base declares
    /// no checks it reaches, `done` once they pass, and [`RED`] when the
    /// last round failed, the rounds ran out of time, or the checks file
    /// does not read.
    pub(super) async fn work_and_check(
        &self,
        step: &StepName,
        instructions: &str,
        outcomes: Vec<Outcome>,
        input: StepInput<'_>,
    ) -> Result<StepReport, BuiltInStepError> {
        let repo = self.pull_requests.records();
        let rounds = repo.check_rounds(input.key).await.map_err(records)?;
        let end_by = self.rounds_end_by(input).await?;

        // No round recorded: the agent's work is still to do, or a retry
        // finds its hand back in the session's history. The work is
        // waited for until the Step's time too, so a Step whose agent is
        // still at work then ends `red`, not failed.
        if rounds.is_empty() {
            let asked = self
                .ask_agent_by(step, instructions, outcomes, input, Some(end_by))
                .await;
            if let Err(BuiltInStepError::Agent { .. }) = &asked
                && Utc::now() >= self.rounds_end_by(input).await?
            {
                return Ok(StepReport {
                    outcome: red(),
                    output: WORK_OUT_OF_TIME.to_string(),
                });
            }
            let work = asked?;
            if work.outcome != Outcome::done() {
                return Ok(work);
            }
            if let Some(ended) = self.first_round(input, &work.output).await? {
                return Ok(ended);
            }
        }

        loop {
            let rounds = repo.check_rounds(input.key).await.map_err(records)?;
            let latest = match next(&rounds) {
                Next::FirstRound => return Err(lost("a recorded round of checks went missing")),
                Next::Passed => return Ok(passed_report(&rounds)),
                // A retry of a Step whose answer was lost ends as it did,
                // without asking the agent again.
                Next::QuotaRanOut => return Ok(red_report(&rounds, Red::QuotaRanOut)),
                Next::Run(latest) | Next::Failed(latest) => latest,
            };
            let declared = RepositoryChecks::parse(&latest.declared)
                .map_err(|_| lost("a recorded checks file does not read"))?;
            let failed_last = matches!(next(&rounds), Next::Failed(_));
            if failed_last && latest.round >= declared.rounds {
                return Ok(red_report(&rounds, Red::NeverPassed));
            }
            if Utc::now() >= end_by {
                return self.end_red(input, Red::OutOfTime).await;
            }

            let asked = if failed_last {
                self.fix_round(step, input, &rounds, latest, &declared, end_by)
                    .await
            } else {
                self.run_round(step, input, latest, end_by).await
            };
            // An agent whose quota ran out cannot go on, and no other Agent
            // takes over its session: the Step ends with what the rounds
            // found, as does one still at work when the rounds' time is up,
            // rather than a failure.
            if let Err(BuiltInStepError::Agent {
                out_of_quota: true, ..
            }) = &asked
            {
                return self.end_red(input, Red::QuotaRanOut).await;
            }
            if let Err(BuiltInStepError::Agent { .. }) = &asked
                && Utc::now() >= self.rounds_end_by(input).await?
            {
                return self.end_red(input, Red::OutOfTime).await;
            }
            asked?;
        }
    }

    /// End a Step whose rounds cannot go on, `why`: a round still open takes
    /// what the agent handed back for its prompt, if it did, and otherwise
    /// ends failed, so it never reads as running; then the Step ends with
    /// what the rounds found. When the agent's quota ran out, the latest
    /// round that did not pass is recorded as ended by it, with its end, so
    /// a retry ends the Step the same way without asking again.
    async fn end_red(
        &self,
        input: StepInput<'_>,
        why: Red,
    ) -> Result<StepReport, BuiltInStepError> {
        let repo = self.pull_requests.records();
        let rounds = repo.check_rounds(input.key).await.map_err(records)?;
        if let Some(open) = rounds.last().filter(|round| round.end.is_none()) {
            let handed = repo
                .handed_back(input.key, open.check_action_id)
                .await
                .map_err(records)?;
            let (outcome, report) = match handed {
                HandedBack::Handed(handed) if handed.outcome == passed() => {
                    (CheckRoundOutcome::Passed, handed.output)
                }
                HandedBack::Handed(handed) if handed.outcome == failed() => {
                    (CheckRoundOutcome::Failed, handed.output)
                }
                _ => (
                    CheckRoundOutcome::Failed,
                    match why {
                        Red::QuotaRanOut => QUOTA_RAN_OUT,
                        Red::OutOfTime | Red::NeverPassed => {
                            "The step's time ran out before the agent reported these checks."
                        }
                    }
                    .to_string(),
                ),
            };
            if matches!(why, Red::QuotaRanOut) && outcome == CheckRoundOutcome::Failed {
                repo.end_check_round_on_quota(input.key, open.round, &report)
                    .await
                    .map_err(records)?;
            } else {
                repo.end_check_round(input.key, open.round, outcome, &report)
                    .await
                    .map_err(records)?;
            }
        } else if matches!(why, Red::QuotaRanOut)
            && let Some(failed) = rounds.last()
        {
            // The fix of a failed round was refused: the round keeps its
            // report, and no fix follows it.
            repo.end_check_round_on_quota(input.key, failed.round, QUOTA_RAN_OUT)
                .await
                .map_err(records)?;
        }

        let rounds = repo.check_rounds(input.key).await.map_err(records)?;
        Ok(match next(&rounds) {
            Next::Passed => passed_report(&rounds),
            _ => red_report(&rounds, why),
        })
    }

    /// When the Step's rounds must end: [`ROUNDS_END_BY`] after the Step
    /// started.
    async fn rounds_end_by(&self, input: StepInput<'_>) -> Result<DateTime<Utc>, BuiltInStepError> {
        let steps = self
            .pull_requests
            .records()
            .list_steps(input.key.run_id)
            .await
            .map_err(records)?;
        let started = steps
            .iter()
            .find(|step| step.seq == input.key.seq)
            .map(|step| step.started_at)
            .ok_or_else(|| lost("the step is not recorded"))?;
        let budget = chrono::TimeDelta::from_std(ROUNDS_END_BY)
            .map_err(|_| lost("the rounds' time does not fit a date"))?;
        Ok(started + budget)
    }

    /// Record the first round of checks on the work the agent handed back
    /// as `work_output`: the base's checks file, and those of its checks
    /// the branch's changes reach. The Step's end instead, when there is
    /// nothing to check or the file does not read.
    async fn first_round(
        &self,
        input: StepInput<'_>,
        work_output: &str,
    ) -> Result<Option<StepReport>, BuiltInStepError> {
        let Some(text) = self.pull_requests.declared_checks(&input).await? else {
            return Ok(Some(StepReport {
                outcome: Outcome::done(),
                output: work_output.to_string(),
            }));
        };
        let declared = match RepositoryChecks::parse(&text) {
            Ok(declared) => declared,
            Err(error) => {
                return Ok(Some(StepReport {
                    outcome: red(),
                    output: fit(
                        &format!("The repository's checks could not run: {error}"),
                        work_output,
                    ),
                }));
            }
        };
        let changed = self.pull_requests.branch_changes(&input).await?;
        let checks = declared.for_changes(changed.as_deref(), &[]);
        if checks.is_empty() {
            return Ok(Some(StepReport {
                outcome: Outcome::done(),
                output: work_output.to_string(),
            }));
        }

        self.record_round(input, 1, work_output, &checks, text)
            .await?;
        Ok(None)
    }

    async fn record_round(
        &self,
        input: StepInput<'_>,
        round: u8,
        work_output: &str,
        checks: &[&RepositoryCheck],
        declared: String,
    ) -> Result<(), BuiltInStepError> {
        let start = CheckRoundStart {
            round,
            work_output: work_output.to_string(),
            checks: checks.iter().map(|check| check.name.clone()).collect(),
            commands: checks.iter().map(|check| check.run.clone()).collect(),
            declared,
            check_action_id: Uuid::now_v7(),
        };
        self.pull_requests
            .records()
            .start_check_round(input.key, &start)
            .await
            .map_err(records)
    }

    /// Have the agent run `round`'s recorded checks, and record how they
    /// went.
    async fn run_round(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        round: &CheckRound,
        answer_by: DateTime<Utc>,
    ) -> Result<(), BuiltInStepError> {
        let answer = self
            .agent
            .follow_up(AgentTurn {
                key: input.key,
                owner: input.owner.clone(),
                role: turn_role(step, input)?,
                action_id: round.check_action_id,
                place: checks_place(round.round),
                prompt: checks_prompt(round),
                outcomes: vec![passed(), failed()],
                answer_by,
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

    /// Send `failed`'s report back to the agent, wait for its fix, and
    /// record the next round on it: the checks the first round ran, and
    /// any others the branch's changes now reach.
    async fn fix_round(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        rounds: &[CheckRound],
        failed: &CheckRound,
        declared: &RepositoryChecks,
        answer_by: DateTime<Utc>,
    ) -> Result<(), BuiltInStepError> {
        let action_id = self
            .pull_requests
            .records()
            .send_back_check_round(input.key, failed.round, Uuid::now_v7())
            .await
            .map_err(records)?
            .ok_or_else(|| lost("a round that failed reads as not failed"))?;
        let report = failed.end.as_ref().map_or("", |end| end.report.as_str());
        let answer = self
            .agent
            .follow_up(AgentTurn {
                key: input.key,
                owner: input.owner.clone(),
                role: turn_role(step, input)?,
                action_id,
                place: checks_place(failed.round) + 1,
                prompt: format!("{CHECKS_FIX_INSTRUCTIONS}{report}\n"),
                outcomes: vec![Outcome::done()],
                answer_by,
            })
            .await
            .map_err(BuiltInStepError::agent)?;
        if answer.outcome != Outcome::done() {
            return Err(BuiltInStepError::UnexpectedOutcome {
                step: step.clone(),
                outcome: answer.outcome,
            });
        }

        let first = rounds
            .first()
            .map_or(&[][..], |first| first.checks.as_slice());
        let changed = self.pull_requests.branch_changes(&input).await?;
        let checks = declared.for_changes(changed.as_deref(), first);
        self.record_round(
            input,
            failed.round + 1,
            &answer.text,
            &checks,
            failed.declared.clone(),
        )
        .await
    }
}

/// How a Step whose rounds passed ends: `done`, with the agent's work, what
/// each fix did, and which checks passed in which round.
fn passed_report(rounds: &[CheckRound]) -> StepReport {
    let last = rounds.last();
    let summary = last.map_or_else(String::new, |last| {
        format!(
            "The repository's checks passed in round {}: {}.",
            last.round,
            last.checks.join(", ")
        )
    });
    StepReport {
        outcome: Outcome::done(),
        output: fit(&summary, &work(rounds)),
    }
}

/// Why a Step's checks did not pass.
enum Red {
    /// The last round the repository allows failed.
    NeverPassed,
    /// The Step's time for rounds ran out first.
    OutOfTime,
    /// The agent's quota ran out first.
    QuotaRanOut,
}

/// How a Step whose checks did not pass ends: [`RED`], saying why, with the
/// last report and the agent's work.
fn red_report(rounds: &[CheckRound], why: Red) -> StepReport {
    let ended = rounds.iter().filter(|round| round.end.is_some()).count();
    let report = rounds
        .iter()
        .rev()
        .find_map(|round| round.end.as_ref())
        .map_or("", |end| end.report.as_str());
    let checks = rounds
        .last()
        .map_or_else(String::new, |round| round.checks.join(", "));
    let last = rounds.last().map_or(0, |round| round.round);
    let summary = match why {
        Red::NeverPassed => {
            format!("The repository's checks still fail in round {last}, their last: {checks}.")
        }
        Red::OutOfTime => format!(
            "The repository's checks did not pass before the step's time ran out, with {ended} of their rounds ended: {checks}."
        ),
        Red::QuotaRanOut => format!(
            "The agent's quota ran out during the repository's checks, with {ended} of their rounds ended: {checks}."
        ),
    };
    // The report takes at most half of what the output holds, so some of
    // the agent's work is kept after it.
    let report = cut(report, MAX_OUTPUT_BYTES / 2);
    StepReport {
        outcome: red(),
        output: fit(&summary, &format!("{report}\n\n{}", work(rounds))),
    }
}

/// The agent's work across the rounds: what it first handed back, then
/// what each fix did.
fn work(rounds: &[CheckRound]) -> String {
    let mut work = String::new();
    for round in rounds {
        if round.round == 1 {
            work.push_str(&round.work_output);
        } else {
            work.push_str(&format!(
                "\n\nAfter round {} failed: {}",
                round.round - 1,
                round.work_output
            ));
        }
    }
    work
}

/// `summary`, then `body` cut to fit what a Step's output holds.
fn fit(summary: &str, body: &str) -> String {
    let joined = format!("{summary}\n\n{body}");
    if joined.len() <= MAX_OUTPUT_BYTES {
        return joined;
    }
    let mut output = cut(&joined, MAX_OUTPUT_BYTES - CUT_MARK.len()).to_string();
    output.push_str(CUT_MARK);
    output
}

/// The prompt that asks the agent to run `round`'s checks.
fn checks_prompt(round: &CheckRound) -> String {
    let mut prompt = format!("{CHECKS_INSTRUCTIONS}Round {}.\n\n", round.round);
    for (name, run) in round.checks.iter().zip(&round.commands) {
        prompt.push_str(&format!("- `{name}`: `{run}`\n"));
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
