//! The Postgres Run repository's flaky CI reruns (`factory_ci_retry`), the
//! jobs quarantined in a repository (`factory_ci_quarantine`), and the
//! known flakes a Run keeps from its Factory Definition.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
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
    // A quarantine made by hand covers its whole job; one its reruns made,
    // only its own failure.
    let mut quarantined = Vec::new();
    let mut quarantined_jobs = Vec::new();
    let rows = sqlx::query!(
        r#"
        SELECT name, failed_step, failure, quarantined_by IS NOT NULL AS "by_hand!"
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
    .await?;
    for row in rows {
        if row.by_hand {
            quarantined_jobs.push(row.name);
        } else {
            quarantined.push(CiFailureIdentity {
                name: row.name,
                failed_step: row.failed_step,
                failure: row.failure,
            });
        }
    }
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
              AND NOT EXISTS (
                  SELECT 1 FROM factory_ci_release x
                  WHERE x.repo_url = $1 AND x.name = j.name AND x.released_at >= w.recorded_at
              )
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
              AND NOT EXISTS (
                  SELECT 1 FROM factory_ci_release x
                  WHERE x.repo_url = $1 AND x.name = t.name AND x.released_at >= t.settled_at
              )
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
        quarantined_jobs,
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
    // Run, and since the job was last released: a release drops the
    // evidence before it. One quarantined already keeps its task and Run;
    // the count and the last pass move on, so it lapses only once its
    // reruns stop passing. A job quarantined whole by hand is extended
    // instead (`extended`), and no narrower failure of it is quarantined
    // beside it, so it files no task. `xmax` is 0 only on a row this
    // statement inserted.
    let made = sqlx::query!(
        r#"
        WITH extended AS (
            UPDATE factory_ci_quarantine
            SET passed_retries = passed_retries + 1, last_passed_at = now()
            WHERE repo_url = $2 AND name = ANY($3) AND quarantined_by IS NOT NULL
            RETURNING name
        )
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
              AND NOT EXISTS (
                  SELECT 1 FROM factory_ci_release x
                  WHERE x.repo_url = $2 AND x.name = job.name AND x.released_at >= t.settled_at
              )
        ) passed
        WHERE passed.retries >= $8
          AND job.name NOT IN (SELECT name FROM extended)
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

pub(super) async fn quarantine_ci_job(
    pool: &PgPool,
    repository: &str,
    name: &str,
    by: &MacroUserIdStr<'static>,
    id: Uuid,
    since: DateTime<Utc>,
) -> Result<bool, PgRunRepositoryErr> {
    // A quarantine by hand covers the whole job, and keeps who made it. It
    // shares its key with a quarantine its reruns made of a failure that
    // named no step and no test: that one becomes the hand's, covering the
    // whole job from then on. One by hand already is left as it is while it
    // holds; one that lapsed starts over, as made now. A row is returned
    // only when the statement inserted, revived or took one over.
    let made = sqlx::query_scalar!(
        r#"
        INSERT INTO factory_ci_quarantine
            (task_id, repo_url, name, passed_retries, quarantined_by)
        VALUES ($1, $2, $3, 0, $4)
        ON CONFLICT (repo_url, name, coalesce(failed_step, ''), coalesce(failure, ''))
        DO UPDATE SET
            quarantined_by = coalesce(factory_ci_quarantine.quarantined_by, EXCLUDED.quarantined_by),
            quarantined_at = CASE WHEN factory_ci_quarantine.last_passed_at < $5
                THEN now() ELSE factory_ci_quarantine.quarantined_at END,
            last_passed_at = greatest(factory_ci_quarantine.last_passed_at, now())
        WHERE factory_ci_quarantine.quarantined_by IS NULL
            OR factory_ci_quarantine.last_passed_at < $5
        RETURNING task_id
        "#,
        id,
        repository,
        name,
        by.as_ref(),
        since,
    )
    .fetch_optional(pool)
    .await?;
    Ok(made.is_some())
}

pub(super) async fn lift_ci_job_quarantines(
    pool: &PgPool,
    repository: &str,
    name: &str,
    by: &MacroUserIdStr<'static>,
) -> Result<u32, PgRunRepositoryErr> {
    // The release is kept, so the evidence before it no longer counts, and
    // the quarantines are deleted through `factory_ci_quarantine_failure_idx`,
    // in one statement.
    let lifted = sqlx::query_scalar!(
        r#"
        WITH released AS (
            INSERT INTO factory_ci_release (repo_url, name, released_by)
            VALUES ($1, $2, $3)
            ON CONFLICT (repo_url, name)
            DO UPDATE SET released_at = now(), released_by = EXCLUDED.released_by
        ),
        lifted AS (
            DELETE FROM factory_ci_quarantine
            WHERE repo_url = $1 AND name = $2
            RETURNING 1
        )
        SELECT count(*) AS "lifted!" FROM lifted
        "#,
        repository,
        name,
        by.as_ref(),
    )
    .fetch_one(pool)
    .await?;
    Ok(u32::try_from(lifted).unwrap_or(u32::MAX))
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
