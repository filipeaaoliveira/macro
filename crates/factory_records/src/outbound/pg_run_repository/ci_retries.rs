//! The Postgres Run repository's flaky CI reruns (`factory_ci_retry`), the
//! jobs quarantined in a repository (`factory_ci_quarantine`), and the
//! known flakes a Run keeps from its Factory Definition.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::PgRunRepositoryErr;
use crate::domain::models::{RunId, StepKey};
use crate::domain::quality::{
    CiFailureIdentity, CiFlakes, CiQuarantine, CiRetry, CiRetryMatch, CiRetryRequest,
    CiRetryResult, FlakyCiFailure, NewCiQuarantine, RunCiReruns, RunCiRetry,
};
use crate::domain::standing_rules::StandingRule;

/// The most failures of other pull requests [`ci_flakes`] reads: the most
/// widespread first.
const FAILURES_READ: i64 = 500;

/// The most quarantined jobs of a repository [`ci_flakes`] reads.
const QUARANTINES_READ: i64 = 1000;

fn retry_match(word: String) -> Result<CiRetryMatch, PgRunRepositoryErr> {
    CiRetryMatch::parse(&word).ok_or(PgRunRepositoryErr::UnknownCiWord(word))
}

pub(super) fn retry_result(
    word: Option<String>,
) -> Result<Option<CiRetryResult>, PgRunRepositoryErr> {
    word.map(|word| CiRetryResult::parse(&word).ok_or(PgRunRepositoryErr::UnknownCiWord(word)))
        .transpose()
}

fn count(value: i32) -> u32 {
    u32::try_from(value).unwrap_or_default()
}

fn id(value: Option<i64>) -> Option<u64> {
    value.and_then(|value| u64::try_from(value).ok())
}

pub(super) async fn keep_known_ci_flakes(
    pool: &PgPool,
    id: RunId,
    names: &[String],
) -> Result<(), PgRunRepositoryErr> {
    sqlx::query!(
        "UPDATE factory_run SET known_ci_flakes = $2 WHERE id = $1",
        id.as_uuid(),
        names,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn ci_flakes(
    pool: &PgPool,
    id: RunId,
    repository: &str,
    pull_request: Option<&str>,
    since: DateTime<Utc>,
) -> Result<CiFlakes, PgRunRepositoryErr> {
    let known = sqlx::query_scalar!(
        r#"SELECT known_ci_flakes AS "known_ci_flakes?" FROM factory_run WHERE id = $1"#,
        id.as_uuid(),
    )
    .fetch_optional(pool)
    .await?
    .flatten()
    .unwrap_or_default();
    let quarantined = sqlx::query!(
        r#"
        SELECT name, failed_step, failure
        FROM factory_ci_quarantine
        WHERE repo_url = $1 AND last_passed_at >= $2
        ORDER BY name, failed_step, failure
        LIMIT $3
        "#,
        repository,
        since,
        QUARANTINES_READ,
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| CiFailureIdentity {
        name: row.name,
        failed_step: row.failed_step,
        failure: row.failure,
    })
    .collect();
    // The failures other pull requests of the repository proved flaky: a
    // rerun of the job passed, or the same Run saw the same head green
    // after the red, with no new commit. Each counts once per pull
    // request; a red no later green proved flaky counts for nothing.
    let failures = sqlx::query!(
        r#"
        SELECT
            name AS "name!",
            failed_step,
            failure,
            count(DISTINCT pull_request)::int AS "pull_requests!"
        FROM (
            SELECT j.name, j.failed_step, j.failure,
                coalesce(lower(r.pull_request_url), r.id::text) AS pull_request
            FROM factory_ci_wait w
            JOIN factory_ci_job j ON j.run_id = w.run_id AND j.seq = w.seq
            JOIN factory_run r ON r.id = w.run_id
            WHERE w.recorded_at >= $3
              AND w.verdict = 'red'
              AND lower(r.repo_url) = $1
              AND r.id <> $2
              AND lower(r.pull_request_url) IS DISTINCT FROM lower($4)
              AND j.result IN ('failed', 'setup')
              AND NOT j.replaced
              AND EXISTS (
                  SELECT 1
                  FROM factory_ci_wait g
                  WHERE g.run_id = w.run_id
                    AND g.seq > w.seq
                    AND g.head_sha = w.head_sha
                    AND g.verdict = 'green'
              )
            UNION ALL
            SELECT t.name, t.failed_step, t.failure,
                coalesce(lower(r.pull_request_url), r.id::text) AS pull_request
            FROM factory_ci_retry t
            JOIN factory_run r ON r.id = t.run_id
            WHERE t.result = 'passed'
              AND t.settled_at >= $3
              AND lower(r.repo_url) = $1
              AND r.id <> $2
              AND lower(r.pull_request_url) IS DISTINCT FROM lower($4)
        ) proved
        GROUP BY name, failed_step, failure
        ORDER BY 4 DESC, 1, 2, 3
        LIMIT $5
        "#,
        repository,
        id.as_uuid(),
        since,
        pull_request,
        FAILURES_READ,
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| FlakyCiFailure {
        name: row.name,
        failed_step: row.failed_step,
        failure: row.failure,
        pull_requests: count(row.pull_requests),
    })
    .collect();
    Ok(CiFlakes {
        known,
        quarantined,
        failures,
    })
}

pub(super) async fn ci_retries(
    pool: &PgPool,
    key: StepKey,
) -> Result<Vec<CiRetry>, PgRunRepositoryErr> {
    sqlx::query!(
        r#"
        SELECT name, head_sha, job_id, workflow_run_id, failed_step, failure, matched, result,
            requested_at, settled_at
        FROM factory_ci_retry
        WHERE run_id = $1 AND seq = $2
        ORDER BY name
        "#,
        key.run_id.as_uuid(),
        i32::from(key.seq),
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok(CiRetry {
            name: row.name,
            head_sha: row.head_sha,
            job_id: u64::try_from(row.job_id).unwrap_or_default(),
            workflow_run_id: id(row.workflow_run_id),
            failed_step: row.failed_step,
            failure: row.failure,
            matched: retry_match(row.matched)?,
            result: retry_result(row.result)?,
            requested_at: row.requested_at,
            settled_at: row.settled_at,
        })
    })
    .collect()
}

/// The reruns the Run `id`'s `ci` Steps asked for, through the reruns'
/// primary key, and the quarantines it made, through
/// `factory_ci_quarantine_run_idx`.
pub(super) async fn run_ci_reruns(
    pool: &PgPool,
    id: RunId,
) -> Result<RunCiReruns, PgRunRepositoryErr> {
    let retries = sqlx::query!(
        r#"
        SELECT seq, name, head_sha, job_id, workflow_run_id, failed_step, failure, matched,
            result, requested_at, settled_at
        FROM factory_ci_retry
        WHERE run_id = $1
        ORDER BY seq, name
        "#,
        id.as_uuid(),
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| {
        Ok(RunCiRetry {
            seq: u16::try_from(row.seq).map_err(|_| PgRunRepositoryErr::OutOfRange(row.seq))?,
            retry: CiRetry {
                name: row.name,
                head_sha: row.head_sha,
                job_id: u64::try_from(row.job_id).unwrap_or_default(),
                workflow_run_id: self::id(row.workflow_run_id),
                failed_step: row.failed_step,
                failure: row.failure,
                matched: retry_match(row.matched)?,
                result: retry_result(row.result)?,
                requested_at: row.requested_at,
                settled_at: row.settled_at,
            },
        })
    })
    .collect::<Result<Vec<_>, PgRunRepositoryErr>>()?;
    let quarantines = sqlx::query_as!(
        QuarantineRow,
        r#"
        SELECT task_id, repo_url, name, failed_step, failure, run_id, passed_retries, filed,
            quarantined_at, last_passed_at
        FROM factory_ci_quarantine
        WHERE run_id = $1
        ORDER BY name, failed_step, failure
        "#,
        id.as_uuid(),
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(CiQuarantine::from)
    .collect();
    Ok(RunCiReruns {
        retries,
        quarantines,
    })
}

pub(super) async fn request_ci_retries(
    pool: &PgPool,
    key: StepKey,
    head_sha: &str,
    retries: &[CiRetryRequest],
) -> Result<Vec<String>, PgRunRepositoryErr> {
    let names: Vec<String> = retries.iter().map(|retry| retry.name.clone()).collect();
    let job_ids: Vec<i64> = retries
        .iter()
        .map(|retry| i64::try_from(retry.job_id).unwrap_or(i64::MAX))
        .collect();
    let workflow_runs: Vec<Option<i64>> = retries
        .iter()
        .map(|retry| {
            retry
                .workflow_run_id
                .and_then(|run| i64::try_from(run).ok())
        })
        .collect();
    let steps: Vec<Option<String>> = retries
        .iter()
        .map(|retry| retry.failed_step.clone())
        .collect();
    let failures: Vec<Option<String>> = retries.iter().map(|retry| retry.failure.clone()).collect();
    let matched: Vec<String> = retries
        .iter()
        .map(|retry| retry.matched.as_str().to_string())
        .collect();
    // Only the rows this call adds come back: an attempt running beside
    // it, which kept them first, is the one that asks GitHub.
    let kept = sqlx::query_scalar!(
        r#"
        INSERT INTO factory_ci_retry (
            run_id, seq, name, head_sha, job_id, workflow_run_id, failed_step, failure,
            matched, rule
        )
        SELECT $1, $2, retry.name, $3, retry.job_id, retry.workflow_run_id,
            retry.failed_step, retry.failure, retry.matched, $10
        FROM unnest($4::text[], $5::bigint[], $6::bigint[], $7::text[], $8::text[], $9::text[])
            AS retry(name, job_id, workflow_run_id, failed_step, failure, matched)
        ON CONFLICT (run_id, seq, name) DO NOTHING
        RETURNING name
        "#,
        key.run_id.as_uuid(),
        i32::from(key.seq),
        head_sha,
        &names,
        &job_ids,
        &workflow_runs as &[Option<i64>],
        &steps as &[Option<String>],
        &failures as &[Option<String>],
        &matched,
        StandingRule::RerunFlakyCi.as_ref(),
    )
    .fetch_all(pool)
    .await?;
    Ok(kept)
}

pub(super) async fn settle_ci_retries(
    pool: &PgPool,
    key: StepKey,
    settled: &[(String, CiRetryResult)],
) -> Result<(), PgRunRepositoryErr> {
    let names: Vec<String> = settled.iter().map(|(name, _)| name.clone()).collect();
    let results: Vec<String> = settled
        .iter()
        .map(|(_, result)| result.as_str().to_string())
        .collect();
    sqlx::query!(
        r#"
        UPDATE factory_ci_retry t
        SET result = settled.result, settled_at = now()
        FROM unnest($3::text[], $4::text[]) AS settled(name, result)
        WHERE t.run_id = $1 AND t.seq = $2 AND t.name = settled.name AND t.result IS NULL
        "#,
        key.run_id.as_uuid(),
        i32::from(key.seq),
        &names,
        &results,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn quarantine_ci_jobs(
    pool: &PgPool,
    id: RunId,
    repository: &str,
    jobs: &[NewCiQuarantine],
    since: DateTime<Utc>,
    threshold: u32,
) -> Result<Vec<String>, PgRunRepositoryErr> {
    let names: Vec<String> = jobs.iter().map(|job| job.failure.name.clone()).collect();
    let steps: Vec<Option<String>> = jobs
        .iter()
        .map(|job| job.failure.failed_step.clone())
        .collect();
    let failures: Vec<Option<String>> =
        jobs.iter().map(|job| job.failure.failure.clone()).collect();
    let task_ids: Vec<Uuid> = jobs.iter().map(|job| job.task_id).collect();
    // Each failure's passed reruns on the repository since `since`, by any
    // Run. One quarantined already keeps its task and Run; the count and
    // the last pass move on, so it lapses only once its reruns stop
    // passing. `xmax` is 0 only on a row this statement inserted.
    let made = sqlx::query!(
        r#"
        INSERT INTO factory_ci_quarantine
            (task_id, repo_url, name, failed_step, failure, run_id, passed_retries)
        SELECT job.task_id, $2, job.name, job.failed_step, job.failure, $1, passed.retries
        FROM unnest($3::text[], $4::text[], $5::text[], $6::uuid[])
            AS job(name, failed_step, failure, task_id)
        CROSS JOIN LATERAL (
            SELECT count(*)::int AS retries
            FROM factory_ci_retry t
            JOIN factory_run r ON r.id = t.run_id
            WHERE t.name = job.name
              AND t.result = 'passed'
              AND t.settled_at >= $7
              AND t.failed_step IS NOT DISTINCT FROM job.failed_step
              AND t.failure IS NOT DISTINCT FROM job.failure
              AND lower(r.repo_url) = $2
        ) passed
        WHERE passed.retries >= $8
        ON CONFLICT (repo_url, name, coalesce(failed_step, ''), coalesce(failure, ''))
        DO UPDATE SET passed_retries = EXCLUDED.passed_retries, last_passed_at = now()
        RETURNING name AS "name!", (xmax = 0) AS "inserted!"
        "#,
        id.as_uuid(),
        repository,
        &names,
        &steps as &[Option<String>],
        &failures as &[Option<String>],
        &task_ids,
        since,
        i32::try_from(threshold).unwrap_or(i32::MAX),
    )
    .fetch_all(pool)
    .await?;
    Ok(made
        .into_iter()
        .filter(|row| row.inserted)
        .map(|row| row.name)
        .collect())
}

struct QuarantineRow {
    task_id: Uuid,
    repo_url: String,
    name: String,
    failed_step: Option<String>,
    failure: Option<String>,
    run_id: Option<Uuid>,
    passed_retries: i32,
    filed: bool,
    quarantined_at: DateTime<Utc>,
    last_passed_at: DateTime<Utc>,
}

impl From<QuarantineRow> for CiQuarantine {
    fn from(row: QuarantineRow) -> Self {
        CiQuarantine {
            repository: row.repo_url,
            failure: CiFailureIdentity {
                name: row.name,
                failed_step: row.failed_step,
                failure: row.failure,
            },
            run_id: row.run_id.map(RunId::from_uuid),
            passed_retries: count(row.passed_retries),
            task_id: row.task_id,
            filed: row.filed,
            quarantined_at: row.quarantined_at,
            last_passed_at: row.last_passed_at,
        }
    }
}

pub(super) async fn unfiled_ci_quarantines(
    pool: &PgPool,
    id: RunId,
) -> Result<Vec<CiQuarantine>, PgRunRepositoryErr> {
    Ok(sqlx::query_as!(
        QuarantineRow,
        r#"
        SELECT task_id, repo_url, name, failed_step, failure, run_id, passed_retries, filed,
            quarantined_at, last_passed_at
        FROM factory_ci_quarantine
        WHERE run_id = $1 AND NOT filed
        ORDER BY name, failed_step, failure
        "#,
        id.as_uuid(),
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(CiQuarantine::from)
    .collect())
}

pub(super) async fn mark_ci_quarantine_filed(
    pool: &PgPool,
    task_id: Uuid,
) -> Result<(), PgRunRepositoryErr> {
    sqlx::query!(
        "UPDATE factory_ci_quarantine SET filed = true WHERE task_id = $1",
        task_id,
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub(super) async fn lift_ci_quarantine(
    pool: &PgPool,
    task_id: Uuid,
) -> Result<bool, PgRunRepositoryErr> {
    let lifted = sqlx::query!(
        "DELETE FROM factory_ci_quarantine WHERE task_id = $1",
        task_id,
    )
    .execute(pool)
    .await?;
    Ok(lifted.rows_affected() > 0)
}

/// The quarantines of `repositories` (their URLs in lower case) whose
/// reruns passed since `since`, by repository and failure, read within the
/// quality read's `transaction`.
pub(super) async fn quarantines_of(
    transaction: &mut sqlx::PgConnection,
    repositories: &[String],
    since: DateTime<Utc>,
) -> Result<Vec<CiQuarantine>, PgRunRepositoryErr> {
    Ok(sqlx::query_as!(
        QuarantineRow,
        r#"
        SELECT task_id, repo_url, name, failed_step, failure, run_id, passed_retries, filed,
            quarantined_at, last_passed_at
        FROM factory_ci_quarantine
        WHERE repo_url = ANY($1) AND last_passed_at >= $2
        ORDER BY repo_url, name, failed_step, failure
        LIMIT $3
        "#,
        repositories,
        since,
        QUARANTINES_READ,
    )
    .fetch_all(transaction)
    .await?
    .into_iter()
    .map(CiQuarantine::from)
    .collect())
}
