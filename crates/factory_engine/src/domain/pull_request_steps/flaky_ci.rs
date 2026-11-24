//! A flaky red, rerun once: under the Standing Rule `rerun_flaky_ci`, when
//! every job that turned the pull request red is a known flake, `ci` asks
//! GitHub to run those jobs again, alone, and waits on, instead of routing
//! `red` to a CI fix. The rule is off unless a team's rules turn it on.
//!
//! A job is a known flake when the Run's Factory Definition names it in
//! `knownFlakes`; when the same failure (its job, step and test, as CI
//! health tells failures apart) is quarantined in the Run's repository, or
//! the job is, by a quarantine that names no step and no test, as one made
//! by hand in Insights; or
//! when that failure proved flaky on [`FLAKY_OTHER_PULL_REQUESTS`] other
//! pull requests of the repository within [`FLAKY_PERIOD`], a rerun of it
//! passing or the same head going green after it. Only GitHub Actions jobs
//! are rerun: a commit status, or another app's check run, cannot be, so a
//! red of one goes to the fix.
//!
//! The evidence is read once per wait. A red is held back while other
//! checks of the head run, so a rerun is asked only once the head's jobs
//! finished and none but flakes failed: at most [`HOLD_PAST_RUN`] after the
//! failed jobs' own workflow runs completed, which GitHub needs before it
//! reruns one of their jobs. A rerun that cannot finish before the wait's
//! deadline is not asked, and one still running at the deadline is red.
//!
//! A Step reruns a job once, and only the attempt that kept the rerun asks
//! GitHub for it. A red on its rerun goes to the fix. A rerun that passes
//! is a passed retry; a failure with [`QUARANTINE_PASSED_RETRIES`] passed
//! retries in its repository within [`FLAKY_PERIOD`] is quarantined, and
//! the Run files a task for it when it ends. A quarantine lapses once no
//! rerun of it passed for [`FLAKY_PERIOD`].

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::quality::{
    CiFailureIdentity, CiFlakes, CiRetry, CiRetryMatch, CiRetryRequest, CiRetryResult, CiVerdict,
    FLAKY_PERIOD, NewCiQuarantine,
};
use factory_records::domain::standing_rules::StandingRule;
use uuid::Uuid;

use super::{PullRequestStepError, PullRequestSteps};
use crate::domain::models::{
    CheckState, CodeRepository, FinishedJob, HeadChecks, JobRerun, StepInput, workflow_run_of,
};
use crate::domain::ports::{MainWatches, PullRequests};
use crate::domain::quality::{
    FLAKY_OTHER_PULL_REQUESTS, QUARANTINE_PASSED_RETRIES, ci_job, failure_identity,
};

/// How long a rerun GitHub took may take to show on the head: until it
/// does, the job reads as the run that failed.
pub(crate) const RERUN_SHOWS_WITHIN: chrono::TimeDelta = chrono::TimeDelta::minutes(2);

/// How long a red is held back, at most, once the workflow runs of its
/// failed jobs completed, for the head's other checks to finish.
pub(crate) const HOLD_PAST_RUN: std::time::Duration = std::time::Duration::from_secs(3 * 60);

/// The time a rerun needs before the wait's deadline: a rerun asked later
/// would most likely still run when the wait ends.
pub(crate) const RERUN_NEEDS: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// What a red of `ci` leads to, once the flaky rule had its say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RedNext {
    /// Hold the red back: its failed jobs may be flaky, and other checks
    /// of the head still run.
    Hold,
    /// Wait on: the rerun of its flaky jobs runs.
    RerunRunning,
    /// Route `red`, with what the rule did for the Step's output; `kept`
    /// when the wait was kept already.
    Red {
        /// What the rule did, for the Step's output; empty when nothing.
        note: String,
        /// Whether the wait was kept already, with its jobs.
        kept: bool,
    },
}

impl RedNext {
    fn red() -> Self {
        Self::Red {
            note: String::new(),
            kept: false,
        }
    }
}

/// A red read of a `ci` wait, as the flaky rule weighs it.
#[derive(Debug, Clone, Copy)]
pub(super) struct RedRead<'a> {
    /// The pull request's page.
    pub(super) pull_request: &'a str,
    /// The checks on its head.
    pub(super) checks: &'a HeadChecks,
    /// The heads the wait read, the last its head now.
    pub(super) heads: &'a [String],
    /// When the wait ends.
    pub(super) deadline: tokio::time::Instant,
    /// The time of the read.
    pub(super) now: DateTime<Utc>,
}

/// What a `ci` wait learned of its flaky reds, kept between its reads, so
/// the rules and the evidence are read once per wait.
#[derive(Debug, Default)]
pub(super) struct FlakyWatch {
    /// Whether the Run's rules have `rerun_flaky_ci` on; none until read.
    rule_on: Option<bool>,
    /// The evidence of flakiness; none until read.
    flakes: Option<CiFlakes>,
    /// The failed jobs of a red, read with their steps and tests, by the
    /// head and the failed check runs' names they were read for.
    failed: Option<(String, BTreeSet<String>, Vec<FinishedJob>)>,
    /// When the failed jobs' workflow runs were first seen completed while
    /// the red was held.
    runs_completed: Option<tokio::time::Instant>,
}

/// Whether any check of `checks` has not finished.
fn still_running(checks: &HeadChecks) -> bool {
    let suite_runs = checks
        .suites
        .iter()
        .any(|suite| suite.state == CheckState::Waiting && suite.check_runs > 0);
    let run_waits = checks
        .runs
        .iter()
        .any(|run| run.state == CheckState::Waiting);
    let status_waits = checks
        .statuses
        .iter()
        .any(|status| status.state == CheckState::Waiting);
    suite_runs || run_waits || status_waits || !checks.held_apps.is_empty()
}

/// Whether every workflow run a failed check run of `checks` is in has
/// completed: no check run of the same workflow run still runs. A failed
/// run whose page names no workflow run is taken as completed.
fn failed_runs_completed(checks: &HeadChecks) -> bool {
    let failed: BTreeSet<u64> = checks
        .runs
        .iter()
        .filter(|run| matches!(run.state, CheckState::Failed(_)))
        .filter_map(|run| run.url.as_deref().and_then(workflow_run_of))
        .collect();
    !checks.runs.iter().any(|run| {
        run.state == CheckState::Waiting
            && run
                .url
                .as_deref()
                .and_then(workflow_run_of)
                .is_some_and(|workflow| failed.contains(&workflow))
    })
}

/// The names of the failed check runs of `checks`; none when a commit
/// status failed, which cannot be rerun, or no check run is named failed.
fn failed_run_names(checks: &HeadChecks) -> Option<BTreeSet<String>> {
    let failed = |state: &CheckState| matches!(state, CheckState::Failed(_));
    if checks.statuses.iter().any(|status| failed(&status.state)) {
        return None;
    }
    let names: BTreeSet<String> = checks
        .runs
        .iter()
        .filter(|run| failed(&run.state))
        .map(|run| run.name.clone())
        .collect();
    (!names.is_empty()).then_some(names)
}

/// Whether the jobs named `names` may be flaky by what `flakes` name, by
/// name alone, before their steps and tests are read.
fn named_in(names: &BTreeSet<String>, flakes: &CiFlakes) -> bool {
    names.iter().all(|name| {
        flakes.known.contains(name)
            || flakes
                .quarantined
                .iter()
                .any(|quarantined| quarantined.name == *name)
            || flakes.failures.iter().any(|failure| failure.name == *name)
    })
}

/// Whether the quarantine of `quarantined` covers `failure`: the same
/// failure, or any of its job's when it names no step and no test.
fn covers(quarantined: &CiFailureIdentity, failure: &CiFailureIdentity) -> bool {
    let whole_job = quarantined.failed_step.is_none() && quarantined.failure.is_none();
    if whole_job {
        quarantined.name == failure.name
    } else {
        quarantined == failure
    }
}

/// Whether the red `jobs`, read with their steps and tests, are all known
/// flakes by `flakes`, failing the way that proved them flaky.
pub(crate) fn may_be_flaky(jobs: &[FinishedJob], flakes: &CiFlakes) -> bool {
    flaky_retries(jobs, flakes).is_some()
}

/// The reruns that take every failed job of `jobs` as a known flake, by
/// `flakes`; none when a job failed that is no known flake, was cancelled,
/// is not a GitHub Actions job, or shares its name with another failed job
/// (a Step reruns a job by its name), or when none failed.
pub(crate) fn flaky_retries(
    jobs: &[FinishedJob],
    flakes: &CiFlakes,
) -> Option<Vec<CiRetryRequest>> {
    let mut retries: Vec<CiRetryRequest> = Vec::new();
    for job in jobs {
        if !matches!(job.state, CheckState::Failed(_)) {
            continue;
        }
        let job_id = job.actions_job_id?;
        let kept = ci_job(job)?;
        if !kept.result.is_red() || retries.iter().any(|retry| retry.name == kept.name) {
            return None;
        }
        let failure = CiFailureIdentity {
            name: kept.name.clone(),
            failed_step: kept.failed_step.clone(),
            failure: kept.failure.clone(),
        };
        let matched = if flakes.known.contains(&kept.name) {
            CiRetryMatch::Known
        } else if flakes
            .quarantined
            .iter()
            .any(|quarantined| covers(quarantined, &failure))
        {
            CiRetryMatch::Quarantined
        } else {
            // The same job, failing the same way, proved flaky on enough
            // other pull requests; a failure with no identity is its own.
            let (failed_step, failed_test) = failure_identity(&kept)?;
            let proved = flakes.failures.iter().any(|other| {
                other.name == kept.name
                    && other.failed_step == failed_step
                    && other.failure == failed_test
                    && other.pull_requests >= FLAKY_OTHER_PULL_REQUESTS
            });
            if !proved {
                return None;
            }
            CiRetryMatch::Record
        };
        retries.push(CiRetryRequest {
            name: kept.name,
            job_id,
            workflow_run_id: job.workflow_run_id,
            failed_step: kept.failed_step,
            failure: kept.failure,
            matched,
        });
    }
    (!retries.is_empty()).then_some(retries)
}

/// Jobs, by `names`, for a person: quoted and joined.
fn job_list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// How the rerun `retry` ended, by the jobs that `finished` on its head at
/// `now`; none while it runs. Its rerun is the job of its name in its
/// workflow run that is not the job that failed.
fn rerun_result(
    retry: &CiRetry,
    finished: &[FinishedJob],
    now: DateTime<Utc>,
) -> Option<CiRetryResult> {
    let job = finished.iter().find(|job| {
        job.name == retry.name
            && retry
                .workflow_run_id
                .is_none_or(|workflow| job.workflow_run_id == Some(workflow))
    })?;
    // Still the run that failed: GitHub has not shown the rerun yet, or
    // never made it.
    if job.actions_job_id == Some(retry.job_id) {
        return (now - retry.requested_at > RERUN_SHOWS_WITHIN).then_some(CiRetryResult::Failed);
    }
    match job.state {
        CheckState::Passed => Some(CiRetryResult::Passed),
        CheckState::Failed(_) => Some(CiRetryResult::Failed),
        CheckState::Waiting => None,
    }
}

impl<G, R, M> PullRequestSteps<G, R, M>
where
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository,
    M: MainWatches,
{
    /// What the red read `red` leads to: held back, a rerun of its flaky
    /// jobs, or the fix. A Run whose rules leave `rerun_flaky_ci` off, or
    /// that kept none, goes to the fix.
    pub(super) async fn after_red(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        red: RedRead<'_>,
        watch: &mut FlakyWatch,
    ) -> Result<RedNext, PullRequestStepError> {
        let RedRead {
            pull_request,
            checks,
            heads,
            deadline,
            now,
        } = red;
        let Some(head) = heads.last() else {
            return Ok(RedNext::red());
        };
        let rule_on = match watch.rule_on {
            Some(on) => on,
            None => {
                let run = self
                    .repo
                    .get_run(input.key.run_id)
                    .await
                    .map_err(PullRequestStepError::records)?;
                let on = run
                    .and_then(|run| run.standing_rules)
                    .is_some_and(|rules| rules.definition.rerun_flaky_ci);
                *watch.rule_on.insert(on)
            }
        };
        if !rule_on {
            return Ok(RedNext::red());
        }
        let retries = self
            .repo
            .ci_retries(input.key)
            .await
            .map_err(PullRequestStepError::records)?;
        if !retries.is_empty() {
            return self
                .rerun_outcome(input, repository, head, &retries, now)
                .await;
        }

        let Some(names) = failed_run_names(checks) else {
            return Ok(RedNext::red());
        };
        let flakes = match watch.flakes.take() {
            Some(flakes) => flakes,
            None => self
                .repo
                .ci_flakes(
                    input.key.run_id,
                    &repository.url().to_lowercase(),
                    Some(pull_request),
                    now - FLAKY_PERIOD,
                )
                .await
                .map_err(PullRequestStepError::records)?,
        };
        let flakes = &*watch.flakes.insert(flakes);
        if !named_in(&names, flakes) {
            return Ok(RedNext::red());
        }
        // The failed jobs' steps and tests, read once for each set of
        // failed jobs on a head.
        let read_for = (head.as_str(), &names);
        let cached = watch
            .failed
            .as_ref()
            .is_some_and(|(at, named, _)| (at.as_str(), named) == read_for);
        if !cached {
            let jobs = self
                .github
                .finished_jobs(input.owner, repository, head, true)
                .await
                .map_err(PullRequestStepError::github)?;
            watch.failed = Some((head.clone(), names.clone(), jobs));
        }
        let failed_jobs = watch
            .failed
            .as_ref()
            .map(|(_, _, jobs)| jobs.as_slice())
            .unwrap_or_default();
        if !may_be_flaky(failed_jobs, flakes) {
            return Ok(RedNext::red());
        }
        if still_running(checks) {
            if !failed_runs_completed(checks) {
                return Ok(RedNext::Hold);
            }
            let completed = *watch
                .runs_completed
                .get_or_insert_with(tokio::time::Instant::now);
            if completed.elapsed() < HOLD_PAST_RUN {
                return Ok(RedNext::Hold);
            }
        }
        if tokio::time::Instant::now() + RERUN_NEEDS > deadline {
            return Ok(RedNext::Red {
                note: format!(
                    "\n\nThe `{}` rule took the failed jobs as known flakes, but too little of the wait was left to rerun them.",
                    StandingRule::RerunFlakyCi
                ),
                kept: false,
            });
        }

        let jobs = self
            .keep_ci_wait(input, repository, heads, CiVerdict::Red, deadline)
            .await;
        let Some(retries) = flaky_retries(&jobs, flakes) else {
            return Ok(RedNext::Red {
                note: String::new(),
                kept: true,
            });
        };
        // Kept before GitHub is asked; only the attempt that kept a rerun
        // asks for it, so a retried or concurrent attempt never asks twice.
        let kept = self
            .repo
            .request_ci_retries(input.key, head, &retries)
            .await
            .map_err(PullRequestStepError::records)?;
        let mut declined = Vec::new();
        for retry in retries.iter().filter(|retry| kept.contains(&retry.name)) {
            let rerun = self
                .github
                .rerun_actions_job(input.owner, repository, retry.job_id)
                .await;
            match rerun {
                Ok(JobRerun::Started) => {}
                Ok(JobRerun::Declined(message)) => declined.push((retry.name.clone(), message)),
                // Not asked again: the rerun is kept, and a later attempt
                // would find it.
                Err(error) => {
                    tracing::warn!(%error, job = retry.name, "a flaky job's rerun could not be asked");
                    declined.push((retry.name.clone(), "GitHub could not be asked.".to_string()));
                }
            }
        }
        if declined.is_empty() {
            return Ok(RedNext::RerunRunning);
        }

        // The red stands while one job cannot run again: every rerun this
        // attempt kept is settled, as none of them decides the Step.
        let settled: Vec<(String, CiRetryResult)> = kept
            .iter()
            .map(|name| (name.clone(), CiRetryResult::Refused))
            .collect();
        self.repo
            .settle_ci_retries(input.key, &settled)
            .await
            .map_err(PullRequestStepError::records)?;
        let reasons: Vec<String> = declined
            .iter()
            .map(|(name, message)| format!("- `{name}`: {message}"))
            .collect();
        Ok(RedNext::Red {
            note: format!(
                "\n\nThe `{}` rule took these as known flakes, but GitHub would not rerun them:\n\n{}",
                StandingRule::RerunFlakyCi,
                reasons.join("\n")
            ),
            kept: true,
        })
    }

    /// What a red on `head` leads to once the Step asked for `retries`:
    /// the fix, when one failed again, never showed, or ran on an earlier
    /// head, or when the red is another job's; otherwise wait on.
    async fn rerun_outcome(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        head: &str,
        retries: &[CiRetry],
        now: DateTime<Utc>,
    ) -> Result<RedNext, PullRequestStepError> {
        let open: Vec<&CiRetry> = retries
            .iter()
            .filter(|retry| retry.result.is_none() && retry.head_sha == head)
            .collect();
        if open.is_empty() {
            return Ok(RedNext::Red {
                note: format!(
                    "\n\nThe `{}` rule reran {} once already in this step.",
                    StandingRule::RerunFlakyCi,
                    job_list(retries.iter().map(|retry| retry.name.as_str()))
                ),
                kept: false,
            });
        }

        let finished = self
            .github
            .finished_jobs(input.owner, repository, head, false)
            .await
            .map_err(PullRequestStepError::github)?;
        let mut settled = Vec::new();
        let mut running = false;
        for retry in open {
            match rerun_result(retry, &finished, now) {
                Some(result) => settled.push((retry.name.clone(), result)),
                None => running = true,
            }
        }
        if !settled.is_empty() {
            self.repo
                .settle_ci_retries(input.key, &settled)
                .await
                .map_err(PullRequestStepError::records)?;
        }

        let failed: Vec<&str> = settled
            .iter()
            .filter(|(_, result)| *result == CiRetryResult::Failed)
            .map(|(name, _)| name.as_str())
            .collect();
        if failed.is_empty() && running {
            return Ok(RedNext::RerunRunning);
        }
        let note = if failed.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nThe `{}` rule reran {} once, as known flaky: {} failed again.",
                StandingRule::RerunFlakyCi,
                job_list(retries.iter().map(|retry| retry.name.as_str())),
                job_list(failed.into_iter())
            )
        };
        Ok(RedNext::Red { note, kept: false })
    }

    /// At the wait's deadline, the reruns of the Step still running on
    /// `head`, settled as failed: the red they were asked for stands. Their
    /// names; none when none runs.
    pub(super) async fn expire_reruns(
        &self,
        input: &StepInput<'_>,
        head: &str,
    ) -> Result<Vec<String>, PullRequestStepError> {
        let retries = self
            .repo
            .ci_retries(input.key)
            .await
            .map_err(PullRequestStepError::records)?;
        let running: Vec<String> = retries
            .into_iter()
            .filter(|retry| retry.result.is_none() && retry.head_sha == head)
            .map(|retry| retry.name)
            .collect();
        if running.is_empty() {
            return Ok(running);
        }
        let settled: Vec<(String, CiRetryResult)> = running
            .iter()
            .map(|name| (name.clone(), CiRetryResult::Failed))
            .collect();
        self.repo
            .settle_ci_retries(input.key, &settled)
            .await
            .map_err(PullRequestStepError::records)?;
        Ok(running)
    }

    /// Once `head` is green, keep the Step's reruns on it as passed
    /// retries, and quarantine each failure whose passed retries reached
    /// [`QUARANTINE_PASSED_RETRIES`]: what the rule did, for the Step's
    /// output.
    pub(super) async fn settle_green_reruns(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        head: &str,
        now: DateTime<Utc>,
    ) -> Result<String, PullRequestStepError> {
        let retries = self
            .repo
            .ci_retries(input.key)
            .await
            .map_err(PullRequestStepError::records)?;
        let passed: Vec<&CiRetry> = retries
            .iter()
            .filter(|retry| retry.result.is_none() && retry.head_sha == head)
            .collect();
        if passed.is_empty() {
            return Ok(String::new());
        }
        let settled: Vec<(String, CiRetryResult)> = passed
            .iter()
            .map(|retry| (retry.name.clone(), CiRetryResult::Passed))
            .collect();
        self.repo
            .settle_ci_retries(input.key, &settled)
            .await
            .map_err(PullRequestStepError::records)?;

        let jobs: Vec<NewCiQuarantine> = passed
            .iter()
            .map(|retry| NewCiQuarantine {
                failure: CiFailureIdentity {
                    name: retry.name.clone(),
                    failed_step: retry.failed_step.clone(),
                    failure: retry.failure.clone(),
                },
                task_id: Uuid::now_v7(),
            })
            .collect();
        let quarantined = self
            .repo
            .quarantine_ci_jobs(
                input.key.run_id,
                &repository.url().to_lowercase(),
                &jobs,
                now - FLAKY_PERIOD,
                QUARANTINE_PASSED_RETRIES,
            )
            .await
            .map_err(PullRequestStepError::records)?;
        let mut note = format!(
            "\n\nThe `{}` rule reran {} once, as known flaky, and they passed.",
            StandingRule::RerunFlakyCi,
            job_list(passed.iter().map(|retry| retry.name.as_str()))
        );
        if !quarantined.is_empty() {
            note.push_str(&format!(
                " Quarantined, as their reruns passed {QUARANTINE_PASSED_RETRIES} times in {} days: {}; a task is filed for each.",
                FLAKY_PERIOD.num_days(),
                job_list(quarantined.iter().map(String::as_str))
            ));
        }
        Ok(note)
    }
}
