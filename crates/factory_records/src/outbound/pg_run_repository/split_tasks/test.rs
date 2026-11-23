use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::PgRunRepository;
use crate::domain::models::{EntryId, NewRun, RunId, RunInsert, StepKey};
use crate::domain::ports::RunRepository;
use crate::domain::split_tasks::SplitTask;
use crate::domain::standing_rules::RunRules;

/// A new Run of Julia's with a started `split` Step at place 3.
async fn run_at_split(repo: &PgRunRepository) -> RunId {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Rebuild the editor",
            user_id: &julia,
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task has no task to miss");
    };
    let entry = EntryId::try_from("split".to_string()).expect("entry");
    repo.start_step(StepKey { run_id: id, seq: 3 }, &entry, 1)
        .await
        .expect("start");
    id
}

/// A `split` Step keeps each child it filed once, at its place, so a
/// retried Step changes nothing; a Run reads its children by place, and
/// another Run's are apart.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_split_step_keeps_each_child_once_by_place(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run = run_at_split(&repo).await;
    let other = run_at_split(&repo).await;
    let split = StepKey {
        run_id: run,
        seq: 3,
    };
    let toolbar = Uuid::from_u128(0x7001);
    let schema = Uuid::from_u128(0x7002);

    repo.record_split_task(split, 1, schema, "Move the schema")
        .await
        .expect("record");
    repo.record_split_task(split, 0, toolbar, "Rebuild the toolbar")
        .await
        .expect("record");
    // A retry's filing of the same child.
    repo.record_split_task(split, 0, toolbar, "Rebuilt again")
        .await
        .expect("record again");

    let children = repo.run_split_tasks(run).await.expect("read");
    let none = repo.run_split_tasks(other).await.expect("read other");

    assert_eq!(
        children,
        vec![
            SplitTask {
                seq: 3,
                place: 0,
                task_id: toolbar,
                name: "Rebuild the toolbar".to_string(),
            },
            SplitTask {
                seq: 3,
                place: 1,
                task_id: schema,
                name: "Move the schema".to_string(),
            },
        ]
    );
    assert!(none.is_empty());
}
