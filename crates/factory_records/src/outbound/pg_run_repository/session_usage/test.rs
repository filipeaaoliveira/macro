use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::session_usage::{SessionUsage, SessionUsageRepository};

fn person(name: &str) -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from(format!("macro|{name}@example.com")).expect("user")
}

fn usage(session_id: Uuid, user: &MacroUserIdStr<'static>) -> SessionUsage {
    SessionUsage {
        session_id,
        user_id: user.clone(),
        bot_id: Uuid::now_v7(),
        input_tokens: 20,
        output_tokens: 2287,
        cache_read_tokens: 273_477,
        cache_write_tokens: 100,
        cost_usd: Some(0.14),
        model: Some("claude-sonnet-5-5".to_string()),
    }
}

/// What a session reported is read back for the person it ran as, and for
/// no one else.
#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_sessions_usage_reads_back_for_its_owner_only(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = person("julia");
    let kept = usage(Uuid::now_v7(), &julia);
    repo.record_session_usage(&kept).await.expect("record");

    let read = repo
        .session_usage(&julia, &[kept.session_id, Uuid::now_v7()])
        .await
        .expect("read");
    let others = repo
        .session_usage(&person("mallory"), &[kept.session_id])
        .await
        .expect("read");

    assert_eq!(read, vec![kept]);
    assert!(others.is_empty());
}

/// A later read of a session's log keeps growing what was kept; one cut
/// short, or one with no cost, never lowers it.
#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_later_record_never_lowers_what_was_kept(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = person("julia");
    let first = usage(Uuid::now_v7(), &julia);
    repo.record_session_usage(&first).await.expect("record");

    let grown = SessionUsage {
        output_tokens: 3297,
        cost_usd: Some(0.17),
        ..first.clone()
    };
    repo.record_session_usage(&grown).await.expect("record");
    let cut_short = SessionUsage {
        input_tokens: 1,
        output_tokens: 1,
        cost_usd: None,
        model: None,
        ..first.clone()
    };
    repo.record_session_usage(&cut_short).await.expect("record");

    let read = repo
        .session_usage(&julia, &[first.session_id])
        .await
        .expect("read");
    assert_eq!(read, vec![grown]);
}

/// A session with no cost reported keeps none.
#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_session_with_no_cost_keeps_none(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let julia = person("julia");
    let kept = SessionUsage {
        cost_usd: None,
        ..usage(Uuid::now_v7(), &julia)
    };
    repo.record_session_usage(&kept).await.expect("record");

    let read = repo
        .session_usage(&julia, &[kept.session_id])
        .await
        .expect("read");
    assert_eq!(read[0].cost_usd, None);
}
