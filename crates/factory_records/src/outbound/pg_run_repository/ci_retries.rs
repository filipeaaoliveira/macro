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
    CiFlakes, CiQuarantine, CiRetry, CiRetryMatch, CiRetryRequest, CiRetryResult, FlakyCiFailure,
    NewCiQuarantine,
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
    let quarantined = sqlx::query_scalar!(
        r#"
        SELECT name
        FROM factory_ci_quarantine
        WHERE repo_url = $1
        ORDER BY name
        LIMIT $2
        "#,
        repository,
        QUARANTINES_READ,
    )
    .fetch_all(pool)
    .await?;
    // The red jobs other Runs on the repository kept, and the jobs they
    // reran, which a passed rerun keeps apart from its wait: each failure
    // counted once per pull request.
    let failures = sqlx::query!(
        r#"
        SELECT
            name AS "name!",
            failed_step,
            failure,
            count(DISTINCT pull_request)::int AS "pull_requests!"
        FROM (
            SELECT j.name, j.failed_step, j.failure,
                coalesce(r.pull_request_url, r.id::text) AS pull_request
            FROM factory_ci_job j
            JOIN factory_ci_wait w ON w.run_id = j.run_id AND w.seq = j.seq
            JOIN factory_run r ON r.id = j.run_id
            WHERE lower(r.repo_url) = $1
              AND r.id <> $2
              AND w.recorded_at >= $3
              AND j.result IN ('failed', 'setup')
              AND NOT j.replaced
            UNION ALL
            SELECT t.name, t.failed_step, t.failure,
                coalesce(r.pull_request_url, r.id::text) AS pull_request
            FROM factory_ci_retry t
            JOIN factory_run r ON r.id = t.run_id
            WHERE lower(r.repo_url) = $1
              AND r.id <> $2
              AND t.requested_at >= $3
        ) failed
        GROUP BY name, failed_step, failure
        ORDER BY 4 DESC, 1, 2, 3
        LIMIT $4
        "#,
        repository,
        id.as_uuid(),
        since,
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
        SELECT name, head_sha, job_id, matched, result, requested_at
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
            matched: retry_match(row.matched)?,
            result: retry_result(row.result)?,
            requested_at: row.requested_at,
        })
    })
    .collect()
}

pub(super) async fn request_ci_retries(
    pool: &PgPool,
    key: StepKey,
    head_sha: &str,
    retries: &[CiRetryRequest],
) -> Result<(), PgRunRepositoryErr> {
    let names: Vec<String> = retries.iter().map(|retry| retry.name.clone()).collect();
    let job_ids: Vec<i64> = retries
        .iter()
        .map(|retry| i64::try_from(retry.job_id).unwrap_or(i64::MAX))
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
    sqlx::query!(
        r#"
        INSERT INTO factory_ci_retry
            (run_id, seq, name, head_sha, job_id, failed_step, failure, matched, rule)
        SELECT $1, $2, retry.name, $3, retry.job_id, retry.failed_step, retry.failure,
            retry.matched, $9
        FROM unnest($4::text[], $5::bigint[], $6::text[], $7::text[], $8::text[])
            AS retry(name, job_id, failed_step, failure, matched)
        ON CONFLICT (run_id, seq, name) DO NOTHING
        "#,
        key.run_id.as_uuid(),
        i32::from(key.seq),
        head_sha,
        &names,
        &job_ids,
        &steps as &[Option<String>],
        &failures as &[Option<String>],
        &matched,
        StandingRule::RerunFlakyCi.as_ref(),
    )
    .execute(pool)
    .await?;
    Ok(())
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
    let names: Vec<String> = jobs.iter().map(|job| job.name.clone()).collect();
    let task_ids: Vec<Uuid> = jobs.iter().map(|job| job.task_id).collect();
    // Each job's passed reruns on the repository since `since`, by any Run.
    let made = sqlx::query_scalar!(
        r#"
        INSERT INTO factory_ci_quarantine (repo_url, name, run_id, passed_retries, task_id)
        SELECT $2, job.name, $1, passed.retries, job.task_id
        FROM unnest($3::text[], $4::uuid[]) AS job(name, task_id)
        CROSS JOIN LATERAL (
            SELECT count(*)::int AS retries
            FROM factory_ci_retry t
            JOIN factory_run r ON r.id = t.run_id
            WHERE t.name = job.name
              AND t.result = 'passed'
              AND t.settled_at >= $5
              AND lower(r.repo_url) = $2
        ) passed
        WHERE passed.retries >= $6
        ON CONFLICT (repo_url, name) DO NOTHING
        RETURNING name
        "#,
        id.as_uuid(),
        repository,
        &names,
        &task_ids,
        since,
        i32::try_from(threshold).unwrap_or(i32::MAX),
    )
    .fetch_all(pool)
    .await?;
    Ok(made)
}

pub(super) async fn unfiled_ci_quarantines(
    pool: &PgPool,
    id: RunId,
) -> Result<Vec<CiQuarantine>, PgRunRepositoryErr> {
    Ok(sqlx::query!(
        r#"
        SELECT repo_url, name, run_id, passed_retries, task_id, filed, quarantined_at
        FROM factory_ci_quarantine
        WHERE run_id = $1 AND NOT filed
        ORDER BY name
        "#,
        id.as_uuid(),
    )
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|row| CiQuarantine {
        repository: row.repo_url,
        name: row.name,
        run_id: RunId::from_uuid(row.run_id),
        passed_retries: count(row.passed_retries),
        task_id: row.task_id,
        filed: row.filed,
        quarantined_at: row.quarantined_at,
    })
    .collect())
}

pub(super) async fn mark_ci_quarantine_filed(
    pool: &PgPool,
    repository: &str,
    name: &str,
) -> Result<(), PgRunRepositoryErr> {
    sqlx::query!(
        "UPDATE factory_ci_quarantine SET filed = true WHERE repo_url = $1 AND name = $2",
        repository,
        name,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// The quarantines of `repositories` (their URLs in lower case), by
/// repository and name, read within the quality read's `transaction`.
pub(super) async fn quarantines_of(
    transaction: &mut sqlx::PgConnection,
    repositories: &[String],
) -> Result<Vec<CiQuarantine>, PgRunRepositoryErr> {
    Ok(sqlx::query!(
        r#"
        SELECT repo_url, name, run_id, passed_retries, task_id, filed, quarantined_at
        FROM factory_ci_quarantine
        WHERE repo_url = ANY($1)
        ORDER BY repo_url, name
        LIMIT $2
        "#,
        repositories,
        QUARANTINES_READ,
    )
    .fetch_all(transaction)
    .await?
    .into_iter()
    .map(|row| CiQuarantine {
        repository: row.repo_url,
        name: row.name,
        run_id: RunId::from_uuid(row.run_id),
        passed_retries: count(row.passed_retries),
        task_id: row.task_id,
        filed: row.filed,
        quarantined_at: row.quarantined_at,
    })
    .collect())
}
