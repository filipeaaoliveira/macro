//! A Run's flaky CI reruns, as its own read shows them: each rerun its `ci`
//! Steps asked for under the `rerun_flaky_ci` rule, with how it ended, and
//! the quarantines its passed reruns made.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use factory_records::domain::quality::{CiQuarantine, CiRetryMatch, CiRetryResult, RunCiReruns};
use serde::Serialize;

use super::CiQuarantined;

/// A failed CI job one of the Run's `ci` Steps reran, taking it for flaky.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CiRerun {
    /// The `ci` Step that reran it.
    pub seq: u16,
    /// The job.
    pub job: String,
    /// The first of its steps that failed, when it was read.
    pub step: Option<String>,
    /// The test that failed, as its check run annotated it.
    pub failure: Option<String>,
    /// Which rerun of the job in the Run it is: 1 for its first, as each
    /// `ci` Step reruns a job at most once.
    pub attempt: u32,
    /// Why it was taken for flaky.
    pub matched: CiRetryMatch,
    /// How it ended; none while it runs.
    pub result: Option<CiRetryResult>,
    /// The commit whose checks it reran.
    pub head_sha: String,
    /// When it was asked for.
    pub requested_at: DateTime<Utc>,
    /// When it ended; none while it runs.
    pub settled_at: Option<DateTime<Utc>>,
}

/// What the Run's reruns came to, as its read shows them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunReruns {
    /// The reruns, by Step then job.
    pub reruns: Vec<CiRerun>,
    /// The failures its passed reruns quarantined in its repository.
    pub quarantines: Vec<CiQuarantined>,
}

impl From<&CiQuarantine> for CiQuarantined {
    fn from(quarantine: &CiQuarantine) -> Self {
        CiQuarantined {
            repository: quarantine.repository.clone(),
            job: quarantine.failure.name.clone(),
            step: quarantine.failure.failed_step.clone(),
            failure: quarantine.failure.failure.clone(),
            passed_retries: quarantine.passed_retries,
            quarantined_at: quarantine.quarantined_at,
            last_passed_at: quarantine.last_passed_at,
            task_id: quarantine.filed.then_some(quarantine.task_id),
        }
    }
}

impl From<RunCiReruns> for RunReruns {
    /// The reruns as kept, by Step then job, each numbered among the
    /// job's reruns in the Run.
    fn from(kept: RunCiReruns) -> Self {
        let mut attempts: HashMap<String, u32> = HashMap::new();
        let reruns = kept
            .retries
            .into_iter()
            .map(|kept| {
                let attempt = attempts.entry(kept.retry.name.clone()).or_default();
                *attempt += 1;
                CiRerun {
                    seq: kept.seq,
                    job: kept.retry.name,
                    step: kept.retry.failed_step,
                    failure: kept.retry.failure,
                    attempt: *attempt,
                    matched: kept.retry.matched,
                    result: kept.retry.result,
                    head_sha: kept.retry.head_sha,
                    requested_at: kept.retry.requested_at,
                    settled_at: kept.retry.settled_at,
                }
            })
            .collect();
        RunReruns {
            reruns,
            quarantines: kept.quarantines.iter().map(CiQuarantined::from).collect(),
        }
    }
}
