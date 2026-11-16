use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;

use super::PgUserDirectory;
use crate::domain::ports::UserDirectory;

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../pg_run_repository/fixtures", scripts("people"))
)]
async fn only_an_account_exists(pool: PgPool) {
    let directory = PgUserDirectory::new(pool);
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid");
    let nobody = MacroUserIdStr::try_from("macro|nobody@example.com".to_string()).expect("valid");

    assert!(directory.user_exists(&julia).await.expect("julia"));
    assert!(!directory.user_exists(&nobody).await.expect("nobody"));
}
