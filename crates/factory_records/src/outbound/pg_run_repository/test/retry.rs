//! A retry of an ended Run: a new Run that keeps what the Run kept as it
//! started, and the Steps before its own, with their Gates and review
//! rounds, and names the Run it retries.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::{entry, insert, julia, key, outcome, planner, session};
use crate::domain::models::{NewRetry, QueueHold, RetryInsert, Run, RunId, RunOutcome, RunRetry};
use crate::domain::ports::RunRepository;
use crate::domain::review_rounds::ReviewRoundRepository as _;
use crate::outbound::pg_run_repository::PgRunRepository;

const HEAD: &str = "1111111111111111111111111111111111111111";

/// Keep the retry `retry`, with no Queue limit.
async fn retry(repo: &PgRunRepository, retry: NewRetry) -> RetryInsert {
    repo.retry_run(retry, QueueHold::UNLIMITED)
        .await
        .expect("retry")
}

/// A Run of Julia's that planned (its plan approved, in a session), wrote,
/// had a review round of one reviewer that found a Blocker, and lost its
/// fix's session: Steps 1 to 3 ended, and Step 4 failed.
async fn lost_at_fix(pool: &PgPool, repo: &PgRunRepository) -> RunId {
    let id = RunId::generate();
    insert(repo, id, "Collapse repeated dashes in slugs").await;
    sqlx::query!(
        "UPDATE factory_run SET repo_url = 'https://github.com/macro/app', \
         base_branch = 'main', pull_request_url = 'https://github.com/macro/app/pull/7', \
         merge_head_sha = $2 WHERE id = $1",
        id.as_uuid(),
        HEAD,
    )
    .execute(pool)
    .await
    .expect("keep its pull request");
    sqlx::query!(
        "INSERT INTO factory_run_arm (run_id, experiment, arm, agents) \
         VALUES ($1, 'writer', 'backup', '{\"writer\": \"backup\"}')",
        id.as_uuid(),
    )
    .execute(pool)
    .await
    .expect("keep its arm");

    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    repo.claim_session(key(id, 1), session(), &planner(), &[outcome("done")])
        .await
        .expect("claim");
    repo.end_step(key(id, 1), &outcome("done"), "The plan")
        .await
        .expect("end");
    repo.open_gate(key(id, 1), "Approve?", &[outcome("approve")])
        .await
        .expect("open");
    repo.answer_gate(
        key(id, 1),
        &outcome("approve"),
        Some("Keep it small"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("answer");
    repo.start_step(key(id, 2), &entry("write"), 1)
        .await
        .expect("start");
    repo.end_step(key(id, 2), &outcome("done"), "Wrote it")
        .await
        .expect("end");
    repo.start_step(key(id, 3), &entry("review"), 1)
        .await
        .expect("start");
    sqlx::query!(
        "INSERT INTO factory_review_round \
         (run_id, seq, round, reviewers, head_sha, diff_base, fix_diff, outcome, ended_at) \
         VALUES ($1, 3, 1, 1, $2, 'main', false, 'changes', now())",
        id.as_uuid(),
        HEAD,
    )
    .execute(pool)
    .await
    .expect("keep its round");
    sqlx::query!(
        "INSERT INTO factory_review_finding \
         (run_id, seq, place, round_seq, path, start_line, end_line, problem, why, kind) \
         VALUES ($1, 3, 1, 3, 'src/slug.rs', 2, 2, 'One dash too many', 'Words run on', \
         'blocker')",
        id.as_uuid(),
    )
    .execute(pool)
    .await
    .expect("keep its blocker");
    repo.end_step(key(id, 3), &outcome("changes"), "One blocker")
        .await
        .expect("end");
    repo.start_step(key(id, 4), &entry("fix"), 1)
        .await
        .expect("start");
    repo.end_step(key(id, 4), &outcome("failed"), "The session was lost")
        .await
        .expect("end");
    repo.end_run(id, RunOutcome::Infrastructure)
        .await
        .expect("end");
    id
}

/// The retry keeps the Run's request, pull request and merge head, its
/// arms, and the Steps before its own, with their outputs, answered Gates
/// and sessions, and the review round and Blocker a fix reads; it names
/// the Run it retries, works on its branch, and has not ended.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_retry_keeps_the_steps_before_its_own_and_names_the_run_it_retries(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let lost = lost_at_fix(&pool, &repo).await;
    let id = RunId::generate();

    let RetryInsert::Inserted(retried) = retry(
        &repo,
        NewRetry {
            id,
            retried: lost,
            first_seq: 4,
        },
    )
    .await
    else {
        panic!("an ended run is retried");
    };

    let read: Run = repo.get_run(id).await.expect("read").expect("kept");
    assert_eq!(read, *retried);
    assert_eq!(
        read.retry,
        Some(RunRetry {
            retried_from: lost,
            branch_of: lost,
            first_seq: 4,
        })
    );
    assert_eq!(read.branch_of(), lost);
    assert_eq!(read.request, "Collapse repeated dashes in slugs");
    assert_eq!(read.end, None);
    assert_eq!(
        read.pull_request_url.as_deref(),
        Some("https://github.com/macro/app/pull/7")
    );
    assert_eq!(
        repo.merge_head(id).await.expect("read").as_deref(),
        Some(HEAD)
    );
    let arms = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM factory_run_arm WHERE run_id = $1"#,
        id.as_uuid(),
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(arms, 1);

    let steps = repo.list_steps(id).await.expect("read");
    let kept: Vec<(u16, &str, &str)> = steps
        .iter()
        .map(|step| {
            let end = step.end.as_ref().expect("a kept step ended");
            (step.seq, step.entry.as_str(), end.output.as_str())
        })
        .collect();
    assert_eq!(
        kept,
        [
            (1, "plan", "The plan"),
            (2, "write", "Wrote it"),
            (3, "review", "One blocker")
        ]
    );
    let answer = steps[0]
        .gate
        .as_ref()
        .and_then(|gate| gate.answer.as_ref())
        .expect("the approval is kept");
    assert_eq!(answer.note.as_deref(), Some("Keep it small"));
    assert_eq!(steps[0].agent, Some(super::agent("primary")));
    let rounds = repo.review_rounds(id).await.expect("read");
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].seq, 3);
    assert_eq!(rounds[0].head_sha.as_deref(), Some(HEAD));
    // The Run it retries keeps its own, as it ended.
    let old = repo.get_run(lost).await.expect("read").expect("kept");
    assert_eq!(
        old.end.map(|end| end.outcome),
        Some(RunOutcome::Infrastructure)
    );
    assert_eq!(old.retry, None);
    assert_eq!(repo.list_steps(lost).await.expect("read").len(), 4);
}

/// A Run that has not ended, or that is missing, is not retried; a Run is
/// retried once; and a retry of a retry works on the first Run's branch.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_is_retried_once_once_ended_and_retries_keep_its_branch(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let running = RunId::generate();
    insert(&repo, running, "Still at it").await;
    let lost = lost_at_fix(&pool, &repo).await;
    let first = RunId::generate();

    let unended = retry(
        &repo,
        NewRetry {
            id: RunId::generate(),
            retried: running,
            first_seq: 1,
        },
    )
    .await;
    let missing = retry(
        &repo,
        NewRetry {
            id: RunId::generate(),
            retried: RunId::from_uuid(Uuid::now_v7()),
            first_seq: 1,
        },
    )
    .await;
    let kept = retry(
        &repo,
        NewRetry {
            id: first,
            retried: lost,
            first_seq: 4,
        },
    )
    .await;
    let again = retry(
        &repo,
        NewRetry {
            id: RunId::generate(),
            retried: lost,
            first_seq: 4,
        },
    )
    .await;
    repo.end_run(first, RunOutcome::Failed).await.expect("end");
    let second = retry(
        &repo,
        NewRetry {
            id: RunId::generate(),
            retried: first,
            first_seq: 4,
        },
    )
    .await;

    assert_eq!(unended, RetryInsert::NotEnded);
    assert_eq!(missing, RetryInsert::NotEnded);
    assert!(matches!(kept, RetryInsert::Inserted(_)), "{kept:?}");
    assert_eq!(again, RetryInsert::Retried);
    let RetryInsert::Inserted(second) = second else {
        panic!("a retry that ended is retried: {second:?}");
    };
    assert_eq!(
        second.retry,
        Some(RunRetry {
            retried_from: first,
            branch_of: lost,
            first_seq: 4,
        })
    );
}

/// A retry is held to its owner's Queue limit, as a new Run is.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_retry_past_the_queue_limit_keeps_nothing(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let lost = lost_at_fix(&pool, &repo).await;

    let full = repo
        .retry_run(
            NewRetry {
                id: RunId::generate(),
                retried: lost,
                first_seq: 4,
            },
            QueueHold {
                limit: Some(0),
                asking_flows: &[],
            },
        )
        .await
        .expect("retry");

    assert!(matches!(full, RetryInsert::QueueFull(_)), "{full:?}");
    let runs = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM factory_run"#)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(runs, 1);
}
