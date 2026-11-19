//! Every agent session a Step worked in, those it left for a new one
//! included: its Run's token budget and Insights count each once.

use chrono::{TimeDelta, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::{entry, insert, julia, key, outcome, planner, session_of};
use crate::domain::insights::{InsightsScope, RunInsightsRepository};
use crate::domain::models::{Outcome, PassedOver, RunId, StepSession};
use crate::domain::ports::RunRepository;
use crate::domain::quality::{QualityScope, RunQualityRepository};
use crate::outbound::pg_run_repository::PgRunRepository;

/// The pass of `name`'s Agent whose session was lost.
fn lost(name: &str) -> PassedOver {
    PassedOver {
        agent: super::agent(name),
        reason: "session_lost".to_string(),
    }
}

async fn sorted_sessions(repo: &PgRunRepository, id: RunId) -> Vec<Uuid> {
    let mut sessions = repo.step_sessions(id).await.expect("read");
    sessions.sort();
    sessions
}

fn sorted(mut sessions: Vec<Uuid>) -> Vec<Uuid> {
    sessions.sort();
    sessions
}

/// Julia's Runs from a day ago on, of every Flow and repository.
fn julia_s_runs() -> InsightsScope {
    let now = Utc::now();
    InsightsScope {
        user_id: julia(),
        from: now - TimeDelta::days(1),
        to: now + TimeDelta::days(1),
        flow: None,
        cap: 1000,
    }
}

/// A Step whose session was lost after its agent worked in it goes on in a
/// new one, and keeps both: the Run's token budget reads both, and so do
/// Insights' cost by entry and by Run.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_step_keeps_the_session_it_lost_with_the_one_it_went_on_in(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the export").await;
    repo.start_step(key(id, 1), &entry("plan"), 1)
        .await
        .expect("start");
    let first = repo
        .claim_session(
            key(id, 1),
            session_of("primary"),
            &planner(),
            &[outcome("done")],
        )
        .await
        .expect("claim")
        .session;
    let second = session_of("primary");
    assert!(
        repo.pass_session(
            key(id, 1),
            first.session_id,
            &planner(),
            &lost("primary"),
            Some(&second),
        )
        .await
        .expect("pass")
    );
    assert_eq!(
        repo.claimed_session(key(id, 1)).await.expect("read"),
        Some(second.clone())
    );

    let both = sorted(vec![first.session_id, second.session_id]);
    assert_eq!(sorted_sessions(&repo, id).await, both);

    let insights = repo.read_insights(&julia_s_runs()).await.expect("insights");
    let by_entry = sorted(
        insights
            .sessions
            .iter()
            .filter(|session| session.run_id == id)
            .inspect(|session| assert_eq!(session.entry, entry("plan")))
            .map(|session| session.session_id)
            .collect(),
    );
    assert_eq!(by_entry, both);
    let quality = repo
        .read_quality(&QualityScope {
            runs: julia_s_runs(),
            repository: None,
        })
        .await
        .expect("quality");
    let run = quality
        .runs
        .iter()
        .find(|run| run.run.id == id)
        .expect("the run is read");
    assert_eq!(run.sessions, both);
}

/// A session counts once however many Steps worked in it and however often
/// it is recorded: the asking Step's session, which the Step that went on
/// shared before losing it, and a pass retried after it was recorded, or
/// one that leaves the Step in its own session.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_session_counts_once_however_often_it_is_kept(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let id = RunId::generate();
    insert(&repo, id, "Plan the import").await;
    let offered = [outcome("done"), Outcome::question()];
    for seq in [1, 2] {
        repo.start_step(key(id, seq), &entry("plan"), 1)
            .await
            .expect("start");
    }
    let asking = repo
        .claim_session(key(id, 1), session_of("primary"), &planner(), &offered)
        .await
        .expect("claim")
        .session;
    repo.end_step(key(id, 1), &Outcome::question(), "CSV or JSON?")
        .await
        .expect("end");
    let going_on = StepSession {
        action_id: Uuid::now_v7(),
        ..asking.clone()
    };
    // A retried claim finds the session claimed and keeps it once.
    for _ in 0..2 {
        repo.claim_session(key(id, 2), going_on.clone(), &planner(), &offered)
            .await
            .expect("claim the asking step's session");
    }

    let afresh = session_of("primary");
    assert!(
        repo.pass_session(
            key(id, 2),
            asking.session_id,
            &planner(),
            &lost("primary"),
            Some(&afresh),
        )
        .await
        .expect("pass")
    );
    // A retried pass finds the Step in its new session and records nothing.
    assert!(
        !repo
            .pass_session(
                key(id, 2),
                asking.session_id,
                &planner(),
                &lost("primary"),
                Some(&session_of("backup")),
            )
            .await
            .expect("pass again")
    );
    // A pass with no Agent left keeps the Step in its session.
    assert!(
        repo.pass_session(
            key(id, 2),
            afresh.session_id,
            &planner(),
            &lost("backup"),
            None,
        )
        .await
        .expect("pass to no one")
    );

    assert_eq!(
        sorted_sessions(&repo, id).await,
        sorted(vec![asking.session_id, afresh.session_id])
    );
    let insights = repo.read_insights(&julia_s_runs()).await.expect("insights");
    assert_eq!(
        insights
            .sessions
            .iter()
            .filter(|session| session.run_id == id)
            .count(),
        2
    );
}

/// The migration keeps each Step's current session among the sessions it
/// used, once, and none for a Step that worked in none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn the_migration_keeps_each_step_s_current_session(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let id = RunId::generate();
    insert(&repo, id, "Plan the thread view").await;
    let (shared, own) = (Uuid::from_u128(0x801), Uuid::from_u128(0x802));

    // The rows stand as they did before the migration: Steps with their
    // sessions, and no sessions kept apart.
    sqlx::raw_sql("DROP TABLE factory_step_used_session")
        .execute(&pool)
        .await
        .expect("drop");
    for (seq, session, action) in [
        (1, Some(shared), 0x901),
        (2, Some(shared), 0x902),
        (3, None, 0),
        (4, Some(own), 0x904),
    ] {
        sqlx::query!(
            "INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, \
                 ended_at, session_id, action_id, offered) \
             VALUES ($1, $2, 'plan', 1, '2026-11-11 09:00Z', 'done', 'Planned', \
                 '2026-11-11 09:30Z', $3, $4, \
                 CASE WHEN $3::uuid IS NULL THEN NULL ELSE '{done}'::text[] END)",
            id.as_uuid(),
            seq,
            session,
            session.map(|_| Uuid::from_u128(action)),
        )
        .execute(&pool)
        .await
        .expect("step recorded");
    }

    let mut tx = pool.begin().await.expect("begin");
    sqlx::raw_sql(include_str!(
        "../../../../../macro_db_client/migrations/20261119080000_factory_step_used_session.sql"
    ))
    .execute(&mut *tx)
    .await
    .expect("migrate");
    tx.commit().await.expect("commit");

    let kept: Vec<(i32, Uuid)> = sqlx::query!(
        "SELECT seq, session_id FROM factory_step_used_session WHERE run_id = $1 ORDER BY seq",
        id.as_uuid(),
    )
    .fetch_all(&pool)
    .await
    .expect("read")
    .into_iter()
    .map(|row| (row.seq, row.session_id))
    .collect();
    assert_eq!(kept, [(1, shared), (2, shared), (4, own)]);
    assert_eq!(sorted_sessions(&repo, id).await, [shared, own]);
}
