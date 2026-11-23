//! The breaks of a write or fix Step's round of checks: once the factory's
//! own run of the round's checks passed, each break the Step's agent
//! reported ([`crate::domain::breaks`]) runs in the round's worktree, on
//! the commit the checks passed on, and its record is kept as it ends, so
//! a retry runs only the breaks still to run. Where the factory runs no
//! command, or no checks, the breaks are recorded unchecked: the agent's
//! word only.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::{DateTime, Utc};
use factory_records::domain::breaks::BreakStatus;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::models::{CheckRound, Role};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use tokio::time::Instant;
use uuid::Uuid;

use super::{BuiltInStepError, BuiltInSteps, records, retry, until, worktree};
use crate::domain::breaks::{BreakRun, MARKER, MAX_BREAKS, Reported, reported};
use crate::domain::check_context::CheckContext;
use crate::domain::checks::kept_output;
use crate::domain::models::StepInput;
use crate::domain::ports::{ChildTasks, MainWatches, PullRequests, StepAgent};

/// How long one run of a break's test may take: it runs twice, without
/// the break and with it.
pub(super) const BREAK_TEST_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// What a break's session command may take beyond its test's two runs:
/// the round's lock's wait, `timeout`'s ten seconds before it kills, for
/// each run, and a minute for the reset, the edit and the file put back.
const BREAK_MARGIN: Duration = Duration::from_secs(worktree::LOCK_WAIT_SECS + 2 * 10 + 60);

/// What a break the Step had no time left for says.
const OUT_OF_TIME: &str = "The step's time ran out before this break ran.";

/// What a break past [`MAX_BREAKS`] says.
const TOO_MANY: &str = "The step reported more breaks than the factory runs (20).";

/// What a break whose session ran no command says.
const NO_COMMAND: &str = "The agent's session runs no command, so the factory did not run this break: it is the agent's word only.";

/// What a break of a Step whose checks the factory did not run says.
pub(super) const NOT_RUN_BY_FACTORY: &str = "The factory ran none of this step's checks (the agent's session runs no command, or the repository declares no checks this change reaches), so it did not run this break: it is the agent's word only.";

/// Where a break runs: the round's worktree, at the commit its checks
/// passed on.
pub(super) struct BreakPlace<'a> {
    /// The Role whose session's machine runs it.
    pub(super) role: &'a Role,
    /// The round's worktree.
    pub(super) round: Uuid,
    /// The commit.
    pub(super) commit: &'a str,
    /// What the checks were told of the change.
    pub(super) context: &'a CheckContext,
    /// What each check's command runs after, as shell: the repository's
    /// setup `env`, so the test runs as the checks did.
    pub(super) environment: &'a str,
}

impl<A, G, R, M, C> BuiltInSteps<A, G, R, M, C>
where
    A: StepAgent,
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository + ReviewRoundRepository,
    M: MainWatches,
    C: ChildTasks,
{
    /// Run each break the Step `input.key`'s agent reported across its
    /// rounds and not recorded yet, at `place`, recording how each went.
    /// A break that would not end before `answer_by` is recorded dropped;
    /// one that would not end in this attempt (`checks_until`), or that
    /// found another run holding the round's lock, fails the attempt for a
    /// retry, which goes on with the breaks still to run. A run that ended
    /// before it said how the test went is recorded interrupted, and the
    /// attempt then fails for a retry once the other breaks ran, so the
    /// round stays open and the break runs once more; interrupted again,
    /// its record stands.
    pub(super) async fn run_breaks(
        &self,
        input: StepInput<'_>,
        place: &BreakPlace<'_>,
        answer_by: DateTime<Utc>,
        checks_until: Instant,
    ) -> Result<(), BuiltInStepError> {
        let repo = self.pull_requests.records();
        let rounds = repo.check_rounds(input.key).await.map_err(records)?;
        let breaks = reported_in(&rounds);
        if breaks.is_empty() {
            return Ok(());
        }
        let (done, interrupted_before) = self.recorded_breaks(input).await?;
        let mut interrupted = Vec::new();
        let mut runnable = 0;
        for reported in &breaks {
            let Reported::Break(read) = reported else {
                if !done.contains(reported.test()) {
                    self.record(input, reported, BreakStatus::Unreadable, None, "")
                        .await?;
                }
                continue;
            };
            runnable += 1;
            if done.contains(reported.test()) {
                continue;
            }
            if runnable > MAX_BREAKS {
                self.record(input, reported, BreakStatus::Dropped, None, TOO_MANY)
                    .await?;
                continue;
            }
            let run_for = BREAK_TEST_TIMEOUT * 2 + BREAK_MARGIN;
            if until(answer_by) < run_for {
                self.record(input, reported, BreakStatus::Dropped, None, OUT_OF_TIME)
                    .await?;
                continue;
            }
            if checks_until.saturating_duration_since(Instant::now()) < run_for {
                return Err(retry(format!(
                    "this attempt has no time left for the break of {}",
                    read.test
                )));
            }
            let nonce = Uuid::now_v7().simple().to_string();
            let ran = self
                .in_checkout(
                    input,
                    place.role,
                    worktree::break_command(
                        place.round,
                        read,
                        place.environment,
                        &nonce,
                        BREAK_TEST_TIMEOUT.as_secs(),
                        worktree::LOCK_WAIT_SECS,
                        place.context,
                    ),
                    run_for,
                )
                .await?;
            let Some(output) = ran else {
                self.record(input, reported, BreakStatus::Unchecked, None, NO_COMMAND)
                    .await?;
                continue;
            };
            if crate::domain::shell_lock::lock_busy(
                output.exit_code,
                &output.stdout,
                &output.stderr,
            ) {
                return Err(retry(format!(
                    "another run of the break of {} still holds the round's lock",
                    read.test
                )));
            }
            // A runtime may return both streams as one.
            let both = format!("{}\n{}", output.stdout, output.stderr);
            let run = BreakRun::read(&output.stdout, &nonce);
            let marker = format!("{MARKER} {nonce}:");
            let shown: Vec<&str> = both
                .lines()
                .filter(|line| !line.trim().starts_with(&marker))
                .collect();
            let mut said = kept_output(shown.join("\n").trim());
            let status = run.status();
            if status == BreakStatus::Interrupted {
                said = format!(
                    "The break's run ended before it said how the test went (exit status {}): a later attempt runs it again.\n{said}",
                    output.exit_code
                );
            }
            self.record(input, reported, status, Some(place.commit), &said)
                .await?;
            if status == BreakStatus::Interrupted && !interrupted_before.contains(&read.test) {
                interrupted.push(read.test.as_str());
            }
        }
        if !interrupted.is_empty() {
            return Err(retry(format!(
                "the break of {} ended before it said how the test went",
                interrupted.join(", ")
            )));
        }
        Ok(())
    }

    /// Record every break the Step `input.key`'s agent reported across its
    /// rounds, or in `work_output` when it has none, as not run by the
    /// factory, `why`: an unreadable one as unreadable.
    pub(super) async fn record_unchecked(
        &self,
        input: StepInput<'_>,
        work_output: Option<&str>,
        why: &str,
    ) -> Result<(), BuiltInStepError> {
        let repo = self.pull_requests.records();
        let rounds = repo.check_rounds(input.key).await.map_err(records)?;
        let breaks = match work_output {
            Some(output) if rounds.is_empty() => reported([output]),
            _ => reported_in(&rounds),
        };
        if breaks.is_empty() {
            return Ok(());
        }
        for reported in &breaks {
            self.record(input, reported, BreakStatus::Unchecked, None, why)
                .await?;
        }
        Ok(())
    }

    /// The tests whose breaks the Step `input.key` recorded, but for an
    /// interrupted run's, which runs again; and those interrupted.
    async fn recorded_breaks(
        &self,
        input: StepInput<'_>,
    ) -> Result<(BTreeSet<String>, BTreeSet<String>), BuiltInStepError> {
        let recorded = self
            .pull_requests
            .records()
            .runs_break_records(&[input.key.run_id])
            .await
            .map_err(records)?;
        let mut done = BTreeSet::new();
        let mut interrupted = BTreeSet::new();
        for record in recorded
            .get(&input.key.run_id)
            .into_iter()
            .flatten()
            .filter(|record| record.seq == input.key.seq)
        {
            if record.status == BreakStatus::Interrupted {
                interrupted.insert(record.test.clone());
            } else {
                done.insert(record.test.clone());
            }
        }
        Ok((done, interrupted))
    }

    async fn record(
        &self,
        input: StepInput<'_>,
        reported: &Reported,
        status: BreakStatus,
        commit: Option<&str>,
        output: &str,
    ) -> Result<(), BuiltInStepError> {
        self.pull_requests
            .records()
            .record_break(input.key, &reported.record(status, commit, output))
            .await
            .map_err(records)
    }
}

/// The breaks reported across `rounds`: the agent's work, then each fix.
fn reported_in(rounds: &[CheckRound]) -> Vec<Reported> {
    reported(rounds.iter().map(|round| round.work_output.as_str()))
}
