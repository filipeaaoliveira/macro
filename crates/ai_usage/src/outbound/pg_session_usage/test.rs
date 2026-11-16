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
            },
            SessionTokens {
                session_id: session(WRITE),
                input: 7,
                output: 3,
                cache_read: 0,
                cache_write: 0,
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
