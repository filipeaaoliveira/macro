use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use macro_uuid::Uuid;
use sqlx::PgPool;

use super::{MAX_SESSIONS, PgSessionUsageReader, SessionUsageError};
use crate::domain::session_usage::{SessionTokens, SessionUsageReader};

const PLAN: &str = "00000000-0000-7000-8000-000000000301";
const WRITE: &str = "00000000-0000-7000-8000-000000000302";
const TEOS: &str = "00000000-0000-7000-8000-000000000303";
const GATEWAY: &str = "00000000-0000-7000-8000-000000000304";

fn session(id: &str) -> Uuid {
    Uuid::parse_str(id).expect("valid session id")
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("session_usage"))
)]
async fn each_session_sums_the_tokens_its_calls_used(pool: PgPool) {
    let reader = PgSessionUsageReader::new(pool);

    let mut read = reader
        .session_tokens(&julia(), &[session(PLAN), session(WRITE), session(GATEWAY)])
        .await
        .expect("read");
    read.sort_by_key(|tokens| tokens.session_id);

    assert_eq!(
        read,
        vec![
            SessionTokens {
                session_id: session(PLAN),
                input: 130,
                output: 25,
                cache_read: 1400,
                cache_write: 50,
                cost_usd: 0.75,
                unpriced_calls: 0,
                reported_by_runtime: false,
                api_key: None,
            },
            SessionTokens {
                session_id: session(WRITE),
                input: 7,
                output: 3,
                cache_read: 0,
                cache_write: 0,
                cost_usd: 0.0,
                unpriced_calls: 1,
                reported_by_runtime: false,
                api_key: None,
            },
        ]
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "fixtures", scripts("session_usage"))
)]
async fn another_persons_session_reads_as_no_usage(pool: PgPool) {
    let reader = PgSessionUsageReader::new(pool);

    let read = reader
        .session_tokens(&julia(), &[session(TEOS)])
        .await
        .expect("read");

    assert_eq!(read, Vec::new());
}

#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_read_of_too_many_sessions_is_refused(pool: PgPool) {
    let reader = PgSessionUsageReader::new(pool);
    let sessions = vec![session(PLAN); MAX_SESSIONS + 1];

    let read = reader.session_tokens(&julia(), &sessions).await;

    assert!(matches!(
        read,
        Err(SessionUsageError::TooManySessions(count)) if count == MAX_SESSIONS + 1
    ));
}

/// Julia's agent Keyed (`b1`), with its own Anthropic API key: only its
/// prefix and a fake ciphertext are stored.
const KEYED: &str = "00000000-0000-7000-8000-0000000000b1";
/// A session of Keyed's.
const KEYED_SESSION: &str = "00000000-0000-7000-8000-000000000305";

/// A session's calls billed to an agent's own API key are summed apart, by
/// the key, beside the session's whole usage.
#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_sessions_calls_on_an_api_key_are_summed_by_the_key(pool: PgPool) {
    sqlx::raw_sql(
        r#"
        INSERT INTO macro_user (id, username, email, stripe_customer_id) VALUES
            ('00000000-0000-7000-8000-000000000001', 'julia@example.com', 'julia@example.com', 'stripe_julia');
        INSERT INTO "User" (id, email, macro_user_id) VALUES
            ('macro|julia@example.com', 'julia@example.com', '00000000-0000-7000-8000-000000000001');
        INSERT INTO bots (id, kind, owner_user_id, name, handle, has_agent) VALUES
            ('00000000-0000-7000-8000-0000000000b1', 'owned', 'macro|julia@example.com', 'Keyed', 'keyed', true);
        INSERT INTO agent_configs (bot_id, instructions, harness, default_model, channel_scope, is_coding)
        VALUES ('00000000-0000-7000-8000-0000000000b1', '', 'in-memory', 'anthropic/claude-sonnet-4-5', 'all', false);
        INSERT INTO agent_api_keys
            (bot_id, provider, key_prefix, key_ciphertext, encryption_version, kms_key_id)
        VALUES ('00000000-0000-7000-8000-0000000000b1', 'anthropic', 'sk-ant-', '\x00', 1, 'test-key');
        INSERT INTO ai_usage (id, feature, user_id, model, input_tokens, output_tokens,
                              cache_read_input_tokens, cache_write_input_tokens,
                              agent_session_id, total, agent_api_key_id)
        VALUES
            ('00000000-0000-7000-8000-000000000411', 'agent_session', 'macro|julia@example.com', 'm', 10, 1, 0, 0,
             '00000000-0000-7000-8000-000000000305', 0.4, '00000000-0000-7000-8000-0000000000b1'),
            ('00000000-0000-7000-8000-000000000412', 'agent_session', 'macro|julia@example.com', 'm', 20, 2, 0, 0,
             '00000000-0000-7000-8000-000000000305', 0.1, '00000000-0000-7000-8000-0000000000b1'),
            ('00000000-0000-7000-8000-000000000413', 'agent_session', 'macro|julia@example.com', 'm', 30, 3, 0, 0,
             '00000000-0000-7000-8000-000000000305', 0.2, NULL);
        "#,
    )
    .execute(&pool)
    .await
    .expect("fixtures");
    let reader = PgSessionUsageReader::new(pool);

    let read = reader
        .session_tokens(&julia(), &[session(KEYED_SESSION)])
        .await
        .expect("read");

    assert_eq!(read.len(), 1);
    assert_eq!(read[0].input, 60);
    assert!((read[0].cost_usd - 0.7).abs() < 1e-6);
    let spend = read[0].api_key.expect("spent on the key");
    assert_eq!(spend.key, session(KEYED));
    assert!((spend.cost_usd - 0.5).abs() < 1e-6);
    assert_eq!(spend.unpriced_calls, 0);
}

/// Macro's own calls about a session, its title and its activity summary,
/// are not its turns: a session with only those reads as no usage, and one
/// with turns too counts only its turns.
#[sqlx::test(migrator = "MACRO_DB_MIGRATIONS")]
async fn a_sessions_title_and_summary_calls_are_not_its_turns(pool: PgPool) {
    sqlx::raw_sql(
        r#"
        INSERT INTO ai_usage (id, feature, user_id, model, input_tokens, output_tokens,
                              cache_read_input_tokens, cache_write_input_tokens,
                              agent_session_id, total)
        VALUES
            ('00000000-0000-7000-8000-000000000421', 'chat_rename', 'macro|julia@example.com', 'm', 50, 5, 0, 0,
             '00000000-0000-7000-8000-000000000306', 0.01),
            ('00000000-0000-7000-8000-000000000422', 'agent_session', 'macro|julia@example.com', 'm', 10, 1, 0, 0,
             '00000000-0000-7000-8000-000000000307', 0.2),
            ('00000000-0000-7000-8000-000000000423', 'agent_activity_summary', 'macro|julia@example.com', 'm', 70, 7, 0, 0,
             '00000000-0000-7000-8000-000000000307', 0.02);
        "#,
    )
    .execute(&pool)
    .await
    .expect("fixtures");
    let reader = PgSessionUsageReader::new(pool);
    let titled_only = session("00000000-0000-7000-8000-000000000306");
    let worked = session("00000000-0000-7000-8000-000000000307");

    let read = reader
        .session_tokens(&julia(), &[titled_only, worked])
        .await
        .expect("read");

    assert_eq!(read.len(), 1);
    assert_eq!(read[0].session_id, worked);
    assert_eq!((read[0].input, read[0].output), (10, 1));
}
