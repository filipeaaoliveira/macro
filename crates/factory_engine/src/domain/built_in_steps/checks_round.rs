//! The checks round of the `write` and `fix` Steps: once the agent hands
//! back its work, the Step runs the checks its repository declares
//! ([`crate::domain::checks`]) on it, and sends a failure back to the agent,
//! until they pass or the repository's rounds run out.
//!
//! Where the agent's session runs in a checkout Macro reaches (a managed
//! sandbox, a Macro computer), the engine runs each check there itself,
//! beside the agent, and reads its exit code: the round is verified, and
//! each check's run is recorded as it ends, so a retry runs only the checks
//! still to run. Elsewhere (a paired machine, a cloud agent) the agent runs
//! the checks in its session and reports how they went. A round the agent
//! was asked to run stays the agent's, and the agent's word never decides a
//! verified round. Each round is recorded before its checks run or its
//! prompt is sent, so a retried Step goes on with the round it was at,
//! never starts one twice, and asks under the prompt ids it recorded.
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

use std::time::{Duration, Instant};

use agent_session::domain::session_command::{SessionCommand, SessionCommandOutput};
use chrono::{DateTime, Utc};
use factory_records::domain::models::{
    CheckResult, CheckRound, CheckRoundOutcome, CheckRoundStart, HandedBack,
    MAX_CHECK_RESULT_OUTPUT_BYTES, MAX_OUTPUT_BYTES, Outcome, Role,
};
use factory_records::domain::ports::RunRepository;
use uuid::Uuid;

use super::{BuiltInStepError, BuiltInSteps, agent_role, cut};
use crate::domain::checks::{CHECKS_FILE, RepositoryCheck, RepositoryChecks};
use crate::domain::flow::StepName;
use crate::domain::models::{AgentCommand, AgentTurn, StepInput, StepReport};
use crate::domain::ports::{PullRequests, StepAgent};
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

/// The longest one check the engine runs may take: past it, the check is
/// stopped, and fails.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// How long one attempt of the Step runs checks, from its start: short of
/// its activity's 30 minutes, as a CI wait is. The checks still to run then
/// go on in the next attempt.
const ATTEMPT_CHECKS_FOR: Duration = Duration::from_secs(25 * 60);

/// What reads the checkout before a round's first check: its uncommitted
/// changes (at most 50 lines of them), then the commit it is at, last, so
/// the end of the output that is kept always holds it.
const CHECKOUT_COMMAND: &str = "changes=$(git status --porcelain --untracked-files=normal) \
     && printf '%s\\n' \"$changes\" | head -n 50 && git rev-parse HEAD";

/// How long reading the checkout may take.
const CHECKOUT_TIMEOUT: Duration = Duration::from_secs(60);

/// The name a verified round records its checkout under when it is not the
/// pushed commit: no check can have it, as a check's name has no space.
const CHECKOUT_RESULT: &str = "pushed commit";

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

impl<A: StepAgent, G: PullRequests, R: RunRepository> BuiltInSteps<A, G, R> {
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
        let checks_until = Instant::now() + ATTEMPT_CHECKS_FOR;
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
                self.run_round(step, input, latest, end_by, checks_until)
                    .await
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
        if let Some(open) = rounds
            .last()
            .filter(|round| round.end.is_none() && round.verified)
        {
            // What the engine read of the checks decides: the agent was
            // never asked, and its word does not count.
            let (outcome, report) = verified_end(open, &open.results);
            repo.end_check_round(input.key, open.round, outcome, &report)
                .await
                .map_err(records)?;
        } else if let Some(open) = rounds.last().filter(|round| round.end.is_none()) {
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

    /// Run `round`'s recorded checks in the agent's checkout, or, where its
    /// session's runtime runs no command, have the agent run them; and
    /// record how they went. A round the agent was already asked to run,
    /// by an earlier attempt, stays the agent's.
    async fn run_round(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        round: &CheckRound,
        answer_by: DateTime<Utc>,
        checks_until: Instant,
    ) -> Result<(), BuiltInStepError> {
        let asked = !round.verified
            && !matches!(
                self.pull_requests
                    .records()
                    .handed_back(input.key, round.check_action_id)
                    .await
                    .map_err(records)?,
                HandedBack::Superseded
            );
        if !asked
            && self
                .verify_round(step, input, round, answer_by, checks_until)
                .await?
        {
            return Ok(());
        }

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

    /// Run each of `round`'s checks not run yet in the agent's checkout,
    /// recording how each ran as it ends, then end the round with what
    /// their exit codes say. Before the round's first check, the checkout
    /// must be the Run's branch as pushed, with nothing uncommitted;
    /// otherwise the round fails saying so, and no check runs. False,
    /// running nothing, when the session's runtime runs no command. A check
    /// starts only with its whole [`CHECK_TIMEOUT`] left in the attempt and
    /// the Step's rounds; with less, the attempt fails, and its retry goes
    /// on with the checks still to run.
    async fn verify_round(
        &self,
        step: &StepName,
        input: StepInput<'_>,
        round: &CheckRound,
        answer_by: DateTime<Utc>,
        checks_until: Instant,
    ) -> Result<bool, BuiltInStepError> {
        let repo = self.pull_requests.records();
        let role = turn_role(step, input)?;
        let mut results = round.results.clone();

        if results.is_empty() {
            match self.checkout(input, &role).await? {
                Checkout::Unreachable => return Ok(false),
                Checkout::Pushed => {}
                Checkout::NotPushed(why) => {
                    let result = CheckResult {
                        name: CHECKOUT_RESULT.to_string(),
                        exit_code: 1,
                        output: kept_output(&why),
                    };
                    repo.record_check_result(input.key, round.round, &result)
                        .await
                        .map_err(records)?;
                    results.push(result);
                }
            }
        }

        let checkout_failed = results.iter().any(|result| result.name == CHECKOUT_RESULT);
        for (name, run) in round.checks.iter().zip(&round.commands) {
            if checkout_failed {
                break;
            }
            if results.iter().any(|result| &result.name == name) {
                continue;
            }
            let left = checks_until
                .saturating_duration_since(Instant::now())
                .min(until(answer_by));
            if left < CHECK_TIMEOUT {
                return Err(BuiltInStepError::Agent {
                    report: rootcause::report!("this attempt has no time left for check {name}")
                        .into_dynamic(),
                    retryable: true,
                    out_of_quota: false,
                });
            }
            let ran = self
                .agent
                .run_check(AgentCommand {
                    key: input.key,
                    owner: input.owner.clone(),
                    role: role.clone(),
                    command: SessionCommand {
                        cwd: ".".to_string(),
                        command: run.clone(),
                        timeout_secs: CHECK_TIMEOUT.as_secs(),
                    },
                })
                .await
                .map_err(BuiltInStepError::agent)?;
            // A runtime that read the checkout runs the checks: one that
            // no longer does is not the session's.
            let Some(output) = ran else {
                return Err(lost("the agent's session no longer runs the checks"));
            };
            let result = CheckResult {
                name: name.clone(),
                exit_code: output.exit_code,
                output: check_output(&output),
            };
            repo.record_check_result(input.key, round.round, &result)
                .await
                .map_err(records)?;
            results.push(result);
        }

        let (outcome, report) = verified_end(round, &results);
        repo.end_check_round(input.key, round.round, outcome, &report)
            .await
            .map_err(records)?;
        Ok(true)
    }

    /// Whether the agent's checkout is the Run's branch as pushed to
    /// GitHub: at its head commit, with nothing uncommitted.
    async fn checkout(
        &self,
        input: StepInput<'_>,
        role: &Role,
    ) -> Result<Checkout, BuiltInStepError> {
        let ran = self
            .agent
            .run_check(AgentCommand {
                key: input.key,
                owner: input.owner.clone(),
                role: role.clone(),
                command: SessionCommand {
                    cwd: ".".to_string(),
                    command: CHECKOUT_COMMAND.to_string(),
                    timeout_secs: CHECKOUT_TIMEOUT.as_secs(),
                },
            })
            .await
            .map_err(BuiltInStepError::agent)?;
        let Some(output) = ran else {
            return Ok(Checkout::Unreachable);
        };
        if output.exit_code != 0 {
            return Ok(Checkout::NotPushed(format!(
                "git could not read your checkout (exit status {}):\n\n```\n{}\n{}\n```",
                output.exit_code,
                output.stdout.trim_end(),
                output.stderr.trim_end()
            )));
        }

        let lines: Vec<&str> = output
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let Some((head, changes)) = lines.split_last() else {
            return Ok(Checkout::NotPushed(
                "git did not say which commit your checkout is at.".to_string(),
            ));
        };
        if !changes.is_empty() {
            return Ok(Checkout::NotPushed(format!(
                "your checkout has changes that are not committed:\n\n```\n{}\n```",
                changes.join("\n")
            )));
        }
        let pushed = self.pull_requests.branch_head(&input).await?;
        match pushed {
            Some(pushed) if pushed == head.trim() => Ok(Checkout::Pushed),
            Some(pushed) => Ok(Checkout::NotPushed(format!(
                "your checkout is at commit {}, and the run's branch on GitHub at {pushed}.",
                head.trim()
            ))),
            None => Ok(Checkout::NotPushed(
                "the run's branch is not on GitHub.".to_string(),
            )),
        }
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

/// What the agent's checkout is, before a verified round's first check.
enum Checkout {
    /// The session's runtime runs no command: the agent runs the checks.
    Unreachable,
    /// The Run's branch as pushed, with nothing uncommitted.
    Pushed,
    /// Anything else, and why, for the agent's fix to read.
    NotPushed(String),
}

/// How a Step whose rounds passed ends: `done`, with the agent's work, what
/// each fix did, and which checks passed in which round.
fn passed_report(rounds: &[CheckRound]) -> StepReport {
    let last = rounds.last();
    let summary = last.map_or_else(String::new, |last| {
        format!(
            "The repository's checks passed in round {}{}: {}.",
            last.round,
            how_run(last),
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
    let how = rounds.last().map_or("", how_run);
    let summary = match why {
        Red::NeverPassed => format!(
            "The repository's checks still fail in round {last}{how}, their last: {checks}."
        ),
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

/// How `round`'s checks ran, as a Step's summary says it: nothing for the
/// agent's report, which was all there was before the engine ran checks.
fn how_run(round: &CheckRound) -> &'static str {
    if round.verified {
        ", run by the factory in the agent's checkout"
    } else {
        ""
    }
}

/// How a verified round ends, from the runs of its checks so far: passed
/// when every check ran and exited 0, failed otherwise. Its report, which
/// the agent's fix reads, holds only the checks that failed or did not
/// run, each with the end of its output.
fn verified_end(round: &CheckRound, results: &[CheckResult]) -> (CheckRoundOutcome, String) {
    if let Some(checkout) = results.iter().find(|result| result.name == CHECKOUT_RESULT) {
        return (
            CheckRoundOutcome::Failed,
            format!(
                "The factory ran no check: {}\n\nCommit all of your work and push the run's branch, so your checkout is the branch's head; the checks then run on that commit.",
                checkout.output.trim_end()
            ),
        );
    }

    let failed: Vec<&CheckResult> = results
        .iter()
        .filter(|result| result.exit_code != 0)
        .collect();
    let not_run: Vec<&String> = round
        .checks
        .iter()
        .filter(|name| !results.iter().any(|result| &result.name == *name))
        .collect();
    if failed.is_empty() && not_run.is_empty() {
        return (
            CheckRoundOutcome::Passed,
            format!(
                "Every check exited with status 0: {}.",
                round.checks.join(", ")
            ),
        );
    }

    let mut report = String::from(
        "The factory ran these checks in your checkout, from the repository's root, and they failed:\n",
    );
    for result in failed {
        let command = round
            .checks
            .iter()
            .position(|name| *name == result.name)
            .and_then(|at| round.commands.get(at))
            .map_or("", String::as_str);
        report.push_str(&format!(
            "\n### `{}`: `{command}` exited with status {}\n\n```\n{}\n```\n",
            result.name,
            result.exit_code,
            result.output.trim_end()
        ));
    }
    if !not_run.is_empty() {
        let names: Vec<&str> = not_run.iter().map(|name| name.as_str()).collect();
        report.push_str(&format!(
            "\nThe step's time ran out before these ran: {}.\n",
            names.join(", ")
        ));
    }
    (
        CheckRoundOutcome::Failed,
        cut(&report, MAX_OUTPUT_BYTES).to_string(),
    )
}

/// What a verified round keeps of a check's run: the end of its output,
/// both streams, and why it stopped when its timeout stopped it.
fn check_output(output: &SessionCommandOutput) -> String {
    let mut text = output.stdout.clone();
    if !output.stderr.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&output.stderr);
    }
    // `timeout` exits 124, or 137 once it had to kill the check.
    if matches!(output.exit_code, 124 | 137) {
        text.push_str(&format!(
            "\n(The check may have run past its {} seconds and been stopped.)",
            CHECK_TIMEOUT.as_secs()
        ));
    }
    kept_output(&text)
}

/// The end of `text` a verified round keeps: without control characters
/// but line breaks and tabs, and at most [`MAX_CHECK_RESULT_OUTPUT_BYTES`]
/// once escaped as JSON, so a round's results stay within what their
/// record holds. Never split inside a character.
fn kept_output(text: &str) -> String {
    let mut kept = Vec::new();
    let mut size = 0;
    for character in text.chars().rev() {
        let escaped = match character {
            '"' | '\\' | '\n' | '\r' | '\t' => 2,
            character if character.is_control() => continue,
            character => character.len_utf8(),
        };
        if size + escaped > MAX_CHECK_RESULT_OUTPUT_BYTES {
            break;
        }
        size += escaped;
        kept.push(character);
    }
    kept.iter().rev().collect()
}

/// How long from now until `at`; nothing once it passed.
fn until(at: DateTime<Utc>) -> Duration {
    (at - Utc::now()).to_std().unwrap_or(Duration::ZERO)
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
