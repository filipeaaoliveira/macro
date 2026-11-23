//! A retry of an ended Run: a new Run that keeps what the Run kept as it
//! started, and the Steps before its own, with their Gates and review
//! rounds, and names the Run it retries.

use chrono::{TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::{entry, insert, julia, key, outcome, planner, session};
use crate::domain::board::{BoardQuery, RunBoardRepository as _};
use crate::domain::evaluation::EvaluationRepository as _;
use crate::domain::gate_replies::GateReplyRepository as _;
use crate::domain::insights::{InsightsScope, RunInsightsRepository as _};
use crate::domain::models::{NewRetry, QueueHold, RetryInsert, Run, RunId, RunOutcome, RunRetry};
use crate::domain::ports::RunRepository;
use crate::domain::quality::{QualityScope, RunQualityRepository as _};
use crate::domain::review_rounds::ReviewRoundRepository as _;
use crate::outbound::pg_run_repository::PgRunRepository;

const HEAD: &str = "1111111111111111111111111111111111111111";

/// A session the plan's Step left for the one it ended in.
const LEFT_SESSION: Uuid = Uuid::from_u128(0x5e55);

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
    sqlx::query!(
        "INSERT INTO factory_step_used_session (run_id, seq, session_id) VALUES ($1, 1, $2)",
        id.as_uuid(),
        LEFT_SESSION,
    )
    .execute(pool)
    .await
    .expect("keep the session it left");
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
    sqlx::query!(
        "INSERT INTO factory_reviewer_answer (run_id, seq, round_seq, read_at) \
         VALUES ($1, 3, 3, now())",
        id.as_uuid(),
    )
    .execute(pool)
    .await
    .expect("keep its reviewer's answer");
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
            created_in_person: false,
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
    let mut sessions = repo.step_sessions(id).await.expect("read");
    let mut kept_sessions = repo.step_sessions(lost).await.expect("read");
    sessions.sort();
    kept_sessions.sort();
    assert!(sessions.contains(&LEFT_SESSION), "{sessions:?}");
    assert_eq!(
        sessions, kept_sessions,
        "the budget counts the kept sessions"
    );
    let answered = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM factory_reviewer_answer WHERE run_id = $1"#,
        id.as_uuid(),
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(answered, 1);
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
            created_in_person: false,
        },
    )
    .await;
    let missing = retry(
        &repo,
        NewRetry {
            id: RunId::generate(),
            retried: RunId::from_uuid(Uuid::now_v7()),
            first_seq: 1,
            created_in_person: false,
        },
    )
    .await;
    let kept = retry(
        &repo,
        NewRetry {
            id: first,
            retried: lost,
            first_seq: 4,
            created_in_person: false,
        },
    )
    .await;
    let again = retry(
        &repo,
        NewRetry {
            id: RunId::generate(),
            retried: lost,
            first_seq: 4,
            created_in_person: false,
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
            created_in_person: false,
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

/// A retried Run names the Run that retried it, in its own read and on the
/// board, so a chain of retries is followed forward without a search; a
/// Run not retried, a new retry among them, names none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_retried_run_names_its_retry(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let lost = lost_at_fix(&pool, &repo).await;
    let first = RunId::generate();
    let second = RunId::generate();

    let RetryInsert::Inserted(kept) = retry(
        &repo,
        NewRetry {
            id: first,
            retried: lost,
            first_seq: 4,
            created_in_person: false,
        },
    )
    .await
    else {
        panic!("an ended Run is retried");
    };
    repo.end_run(first, RunOutcome::Failed).await.expect("end");
    retry(
        &repo,
        NewRetry {
            id: second,
            retried: first,
            first_seq: 4,
            created_in_person: false,
        },
    )
    .await;
    let read = |id| {
        let repo = &repo;
        async move {
            repo.get_run(id)
                .await
                .expect("read")
                .expect("run")
                .retried_by
        }
    };
    let board: Vec<(RunId, Option<RunId>)> = repo
        .board_runs(&BoardQuery {
            user_id: julia(),
            team_id: None,
            state: None,
            flow: None,
            repo_url: None,
            project_id: None,
            search: None,
            built_in_merge_entries: vec!["merge".to_string()],
            after: None,
            limit: 50,
        })
        .await
        .expect("board")
        .into_iter()
        .map(|listed| (listed.run.id, listed.run.retried_by))
        .collect();

    assert_eq!(kept.retried_by, None);
    assert_eq!(read(lost).await, Some(first));
    assert_eq!(read(first).await, Some(second));
    assert_eq!(read(second).await, None);
    assert_eq!(
        board,
        [(second, None), (first, Some(second)), (lost, Some(first))]
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
                created_in_person: false,
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

/// What a retry kept counts once, on the Run that made it: Insights, the
/// quality view and the evaluation tallies leave a retry's kept Steps,
/// Gates, sessions and review rounds out, and count a Run and its retry as
/// one attempt of their arm, the retry.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn what_a_retry_kept_counts_once_on_the_run_that_made_it(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let lost = lost_at_fix(&pool, &repo).await;
    let id = RunId::generate();
    let RetryInsert::Inserted(_) = retry(
        &repo,
        NewRetry {
            id,
            retried: lost,
            first_seq: 4,
            created_in_person: false,
        },
    )
    .await
    else {
        panic!("an ended run is retried");
    };
    repo.start_step(key(id, 4), &entry("fix"), 1)
        .await
        .expect("start");
    repo.end_step(key(id, 4), &outcome("done"), "Fixed it")
        .await
        .expect("end");
    repo.end_run(id, RunOutcome::Success).await.expect("end");
    let now = Utc::now();
    let scope = InsightsScope {
        user_id: julia(),
        from: now - TimeDelta::days(1),
        to: now + TimeDelta::days(1),
        flow: None,
        cap: 1000,
    };

    let insights = repo.read_insights(&scope).await.expect("insights");
    let quality = repo
        .read_quality(&QualityScope {
            runs: scope.clone(),
            repository: None,
        })
        .await
        .expect("quality");
    let evaluation = repo.read_evaluation(&scope).await.expect("evaluation");

    let steps: Vec<(&str, u32)> = insights
        .entries
        .iter()
        .map(|entry| (entry.entry.as_str(), entry.steps))
        .collect();
    assert_eq!(
        steps,
        [("fix", 2), ("plan", 1), ("review", 1), ("write", 1)],
        "each kept step counts once"
    );
    let retry_sessions = insights
        .sessions
        .iter()
        .filter(|session| session.run_id == id)
        .count();
    assert_eq!(
        retry_sessions, 0,
        "the kept sessions count on the retried run"
    );
    let retry_quality = quality
        .runs
        .iter()
        .find(|run| run.run.id == id)
        .expect("the retry");
    assert!(retry_quality.sessions.is_empty());
    assert!(!retry_quality.person_answered);
    assert_eq!(quality.review_rounds.len(), 1);
    assert_eq!(quality.blockers.len(), 1);
    assert_eq!(quality.reviewers.len(), 1);
    let arms: Vec<(u32, u32)> = evaluation
        .arms
        .iter()
        .map(|arm| (arm.runs, arm.succeeded))
        .collect();
    assert_eq!(arms, [(1, 1)], "a run and its retry are one attempt");
    assert_eq!(insights.retried, [lost]);

    // A retry outside the period leaves the attempt to the Run it retried.
    sqlx::query!(
        "UPDATE factory_run SET created_at = now() + interval '2 days' WHERE id = $1",
        id.as_uuid(),
    )
    .execute(&pool)
    .await
    .expect("move the retry out of the period");
    let evaluation = repo.read_evaluation(&scope).await.expect("evaluation");
    let arms: Vec<(u32, u32)> = evaluation
        .arms
        .iter()
        .map(|arm| (arm.runs, arm.succeeded))
        .collect();
    assert_eq!(
        arms,
        [(1, 0)],
        "the attempt is counted by the run in the period"
    );
}

/// The retry keeps the break records of the Steps it keeps, as they were,
/// and none of a Step it runs again.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_retry_keeps_the_break_records_of_its_kept_steps(pool: PgPool) {
    use crate::domain::breaks::{BreakStatus, NewBreakRecord};

    let repo = PgRunRepository::new(pool.clone());
    let lost = lost_at_fix(&pool, &repo).await;
    let record = |test: &str, status| NewBreakRecord {
        test: test.to_string(),
        file: "src/slug.rs".to_string(),
        old_text: "'-'".to_string(),
        new_text: "'_'".to_string(),
        command: "cargo test slug".to_string(),
        status,
        checked_commit: Some(HEAD.to_string()),
        output: "1 failed".to_string(),
    };
    repo.record_break(key(lost, 2), &record("dashes", BreakStatus::Verified))
        .await
        .expect("record the write's");
    repo.record_break(key(lost, 4), &record("ends", BreakStatus::Interrupted))
        .await
        .expect("record the lost fix's");
    let id = RunId::generate();

    let RetryInsert::Inserted(_) = retry(
        &repo,
        NewRetry {
            id,
            retried: lost,
            first_seq: 4,
            created_in_person: false,
        },
    )
    .await
    else {
        panic!("an ended run is retried");
    };

    let kept = repo.runs_break_records(&[id, lost]).await.expect("read");
    let tests: Vec<(u16, &str, BreakStatus)> = kept[&id]
        .iter()
        .map(|record| (record.seq, record.test.as_str(), record.status))
        .collect();
    assert_eq!(tests, [(2, "dashes", BreakStatus::Verified)]);
    assert_eq!(kept[&id][0].recorded_at, kept[&lost][0].recorded_at);
    assert_eq!(kept[&id][0].checked_commit.as_deref(), Some(HEAD));
    assert_eq!(kept[&lost].len(), 2);
}

/// A Gate a chain of retries kept is the owner's answer once, on the Run
/// that asked it: past answers and the replay's answered Gates leave out
/// each retry's copy below its `first_seq`, and read the Gates a retry
/// asked itself.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_kept_gate_is_answered_once_down_a_chain_of_retries(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let lost = lost_at_fix(&pool, &repo).await;
    let first = RunId::generate();
    let RetryInsert::Inserted(_) = retry(
        &repo,
        NewRetry {
            id: first,
            retried: lost,
            first_seq: 4,
            created_in_person: false,
        },
    )
    .await
    else {
        panic!("an ended run is retried");
    };
    // The first retry asks a plan Gate of its own, and fails.
    repo.start_step(key(first, 4), &entry("plan"), 2)
        .await
        .expect("start");
    repo.end_step(key(first, 4), &outcome("done"), "A smaller plan")
        .await
        .expect("end");
    repo.open_gate(
        key(first, 4),
        "Approve?",
        &[outcome("approve"), outcome("revise")],
    )
    .await
    .expect("open");
    repo.answer_gate(
        key(first, 4),
        &outcome("revise"),
        Some("Smaller still"),
        &julia(),
        None,
        None,
    )
    .await
    .expect("answer");
    repo.end_run(first, RunOutcome::Failed).await.expect("end");
    let second = RunId::generate();
    let RetryInsert::Inserted(_) = retry(
        &repo,
        NewRetry {
            id: second,
            retried: first,
            first_seq: 5,
            created_in_person: false,
        },
    )
    .await
    else {
        panic!("a retry that ended is retried");
    };
    let asking = RunId::generate();
    insert(&repo, asking, "Next").await;

    let past = repo
        .past_answers(&julia(), &entry("plan"), asking, 10)
        .await
        .expect("past");
    let answers: Vec<(&str, Option<&str>)> = past
        .iter()
        .map(|answer| (answer.answer.as_str(), answer.note.as_deref()))
        .collect();
    assert_eq!(
        answers,
        [
            ("revise", Some("Smaller still")),
            ("approve", Some("Keep it small"))
        ],
        "each answer once, newest first"
    );
    let answered: Vec<(RunId, u16)> = repo
        .answered_gates(10)
        .await
        .expect("answered")
        .iter()
        .map(|gate| (gate.run_id, gate.seq))
        .collect();
    assert_eq!(answered, [(lost, 1), (first, 4)]);
}
