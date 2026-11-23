//! A flaky red, rerun once: under the Standing Rule `rerun_flaky_ci`, when
//! every job that turned the pull request red is a known flake, `ci` asks
//! GitHub to run those jobs again, alone, and waits on, instead of routing
//! `red` to a CI fix.
//!
//! A job is a known flake when the Run's Factory Definition names it in
//! `knownFlakes`, when it is quarantined in the Run's repository, or when
//! the same failure (its step and its test, as CI health tells failures
//! apart) turned other pull requests of the repository red within
//! [`FLAKY_PERIOD`]. Only GitHub Actions jobs are rerun: a commit status,
//! or another app's check run, cannot be, so a red of one goes to the fix.
//!
//! A red is held back while other checks of the head still run, when its
//! failed jobs may be flaky, so a rerun is asked only once every job
//! finished and none but flakes failed. A Step reruns a job once: a red on
//! its rerun goes to the fix. A rerun that passes is a passed retry; a job
//! with [`QUARANTINE_PASSED_RETRIES`] passed retries in its repository
//! within [`FLAKY_PERIOD`] is quarantined, and the Run files a task for it
//! when it ends.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::MergeQueueRepository;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::quality::{
    CiFlakes, CiRetry, CiRetryMatch, CiRetryRequest, CiRetryResult, CiVerdict, NewCiQuarantine,
};
use factory_records::domain::standing_rules::StandingRule;
use uuid::Uuid;

use super::{PullRequestStepError, PullRequestSteps};
use crate::domain::models::{
    CheckState, CodeRepository, FinishedJob, HeadChecks, JobRerun, StepInput,
};
use crate::domain::ports::{MainWatches, PullRequests};
use crate::domain::quality::{
    FLAKY_OTHER_PULL_REQUESTS, FLAKY_PERIOD, QUARANTINE_PASSED_RETRIES, ci_job, failure_identity,
};

/// How long a rerun GitHub took may take to show on the head: until it
/// does, the job reads as the run that failed.
const RERUN_SHOWS_WITHIN: chrono::TimeDelta = chrono::TimeDelta::minutes(2);

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

/// Whether any check of `checks` has not finished.
fn still_running(checks: &HeadChecks) -> bool {
    let suite_runs = checks
        .suites
        .iter()
        .any(|suite| suite.state == CheckState::Waiting && suite.check_runs > 0);
    let run_waits = checks.runs.iter().any(|run| run.state == CheckState::Waiting);
    let status_waits = checks
        .statuses
        .iter()
        .any(|status| status.state == CheckState::Waiting);
    suite_runs || run_waits || status_waits || !checks.held_apps.is_empty()
}

/// Whether the red `checks` may be flaky by what `flakes` name: a failed
/// check run of each failed suite, each named in `flakes`, and no failed
/// commit status, which cannot be rerun.
fn may_be_flaky(checks: &HeadChecks, flakes: &CiFlakes) -> bool {
    let failed = |state: &CheckState| matches!(state, CheckState::Failed(_));
    if checks.statuses.iter().any(|status| failed(&status.state)) {
        return false;
    }
    let failed_runs: Vec<&str> = checks
        .runs
        .iter()
        .filter(|run| failed(&run.state))
        .map(|run| run.name.as_str())
        .collect();
    let named = |name: &str| {
        flakes.known.iter().any(|known| known == name)
            || flakes.quarantined.iter().any(|quarantined| quarantined == name)
            || flakes.failures.iter().any(|failure| failure.name == name)
    };
    !failed_runs.is_empty() && failed_runs.into_iter().all(named)
}

/// The reruns that take every failed job of `jobs` as a known flake, by
/// `flakes`; none when a job failed that is no known flake, was cancelled,
/// or is not a GitHub Actions job, or when none failed.
pub(crate) fn flaky_retries(jobs: &[FinishedJob], flakes: &CiFlakes) -> Option<Vec<CiRetryRequest>> {
    let mut retries = Vec::new();
    for job in jobs {
        if !matches!(job.state, CheckState::Failed(_)) {
            continue;
        }
        let job_id = job.actions_job_id?;
        let kept = ci_job(job)?;
        if !kept.result.is_red() {
            return None;
        }
        let matched = if flakes.known.contains(&kept.name) {
            CiRetryMatch::Known
        } else if flakes.quarantined.contains(&kept.name) {
            CiRetryMatch::Quarantined
        } else {
            // The same job, failing the same way, on enough other pull
            // requests; a failure with no identity is its own.
            let (failed_step, failure) = failure_identity(&kept)?;
            let recorded = flakes.failures.iter().any(|other| {
                other.name == kept.name
                    && other.failed_step == failed_step
                    && other.failure == failure
                    && other.pull_requests >= FLAKY_OTHER_PULL_REQUESTS
            });
            if !recorded {
                return None;
            }
            CiRetryMatch::Record
        };
        retries.push(CiRetryRequest {
            name: kept.name,
            job_id,
            failed_step: kept.failed_step,
            failure: kept.failure,
            matched,
        });
    }
    (!retries.is_empty()).then_some(retries)
}

/// The jobs of `retries`, for a person: their names, quoted and joined.
fn job_list<'a>(names: impl Iterator<Item = &'a str>) -> String {
    names
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl<G, R, M> PullRequestSteps<G, R, M>
where
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository,
    M: MainWatches,
{
    /// What the red `checks` on the last of `heads` lead to, at `now`:
    /// held back, a rerun of its flaky jobs, or the fix. A Run whose rules
    /// leave `rerun_flaky_ci` off, or that kept none, goes to the fix.
    pub(super) async fn after_red(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        checks: &HeadChecks,
        heads: &[String],
        deadline: tokio::time::Instant,
        now: DateTime<Utc>,
    ) -> Result<RedNext, PullRequestStepError> {
        let Some(head) = heads.last() else {
            return Ok(RedNext::red());
        };
        let run = self
            .repo
            .get_run(input.key.run_id)
            .await
            .map_err(PullRequestStepError::records)?;
        let rule_on = run
            .and_then(|run| run.standing_rules)
            .is_some_and(|rules| rules.definition.rerun_flaky_ci);
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

        let repository_url = repository.url().to_lowercase();
        let flakes = self
            .repo
            .ci_flakes(input.key.run_id, &repository_url, now - FLAKY_PERIOD)
            .await
            .map_err(PullRequestStepError::records)?;
        if !may_be_flaky(checks, &flakes) {
            return Ok(RedNext::red());
        }
        if still_running(checks) {
            return Ok(RedNext::Hold);
        }
        let jobs = self
            .keep_ci_wait(input, repository, heads, CiVerdict::Red, deadline)
            .await;
        let Some(retries) = flaky_retries(&jobs, &flakes) else {
            return Ok(RedNext::Red {
                note: String::new(),
                kept: true,
            });
        };

        // Kept before GitHub is asked, so a retried Step never reruns a
        // job twice.
        self.repo
            .request_ci_retries(input.key, head, &retries)
            .await
            .map_err(PullRequestStepError::records)?;
        let mut declined = Vec::new();
        for retry in &retries {
            let rerun = self
                .github
                .rerun_actions_job(input.owner, repository, retry.job_id)
                .await
                .map_err(PullRequestStepError::github)?;
            if let JobRerun::Declined(message) = rerun {
                declined.push((retry.name.clone(), message));
            }
        }
        if declined.is_empty() {
            return Ok(RedNext::RerunRunning);
        }

        let settled: Vec<(String, CiRetryResult)> = declined
            .iter()
            .map(|(name, _)| (name.clone(), CiRetryResult::Refused))
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
            let job = finished.iter().find(|job| job.name == retry.name);
            let result = match job {
                // Not finished: its rerun runs.
                None => None,
                // Still the run that failed: GitHub has not shown the
                // rerun yet, or never made it.
                Some(job) if job.actions_job_id == Some(retry.job_id) => {
                    (now - retry.requested_at > RERUN_SHOWS_WITHIN).then_some(CiRetryResult::Failed)
                }
                Some(job) => match job.state {
                    CheckState::Passed => Some(CiRetryResult::Passed),
                    CheckState::Failed(_) => Some(CiRetryResult::Failed),
                    CheckState::Waiting => None,
                },
            };
            match result {
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

    /// Once `head` is green, keep the Step's reruns on it as passed
    /// retries, and quarantine each job whose passed retries reached
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
        let passed: Vec<String> = retries
            .iter()
            .filter(|retry| retry.result.is_none() && retry.head_sha == head)
            .map(|retry| retry.name.clone())
            .collect();
        if passed.is_empty() {
            return Ok(String::new());
        }
        let settled: Vec<(String, CiRetryResult)> = passed
            .iter()
            .map(|name| (name.clone(), CiRetryResult::Passed))
            .collect();
        self.repo
            .settle_ci_retries(input.key, &settled)
            .await
            .map_err(PullRequestStepError::records)?;

        let jobs: Vec<NewCiQuarantine> = passed
            .iter()
            .map(|name| NewCiQuarantine {
                name: name.clone(),
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
            job_list(passed.iter().map(String::as_str))
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
