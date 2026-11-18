//! The Factory Definition a team names, and the one a Run keeps: its
//! repository, the commit it read and its Agents.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::{insert, julia};
use crate::domain::models::{
    KeptFlow, NewRun, NewRunDefinition, RunDefinition, RunId, RunInsert,
};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules};
use crate::outbound::pg_run_repository::PgRunRepository;

const REPO_URL: &str = "https://github.com/macro/factory-definition";

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// A team names a Definition, names another in its place, and names none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_team_names_its_definition(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let team_id = Uuid::parse_str("00000000-0000-7000-8000-000000000201").expect("valid");
    assert_eq!(repo.team_definition(team_id).await.expect("read"), None);

    repo.set_team_definition(team_id, Some("https://github.com/macro/old"))
        .await
        .expect("name");
    repo.set_team_definition(team_id, Some(REPO_URL))
        .await
        .expect("name another");
    assert_eq!(
        repo.team_definition(team_id)
            .await
            .expect("read")
            .as_deref(),
        Some(REPO_URL)
    );

    repo.set_team_definition(team_id, None)
        .await
        .expect("name none");
    assert_eq!(repo.team_definition(team_id).await.expect("read"), None);
}

/// A Run keeps the Definition it started under: the repository, the commit
/// it read, its rules' source, and the Agents its Steps take. A Run started
/// under none keeps none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_keeps_the_definition_commit_it_read(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let agents = serde_json::json!({
        "agents": { "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" } },
        "roles": { "writer": ["primary"] }
    });
    let id = RunId::generate();
    let RunInsert::Inserted(inserted) = repo
        .insert_run(NewRun {
            id,
            flow: "notes",
            request: "Note it",
            user_id: &julia(),
            task_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules {
                definition: OrchestratorDefinition::DEFAULT,
                source: RulesSource::Definition,
                project_id: None,
            },
            automation: None,
            definition: Some(NewRunDefinition {
                repo_url: REPO_URL,
                commit: COMMIT,
                agents: &agents,
            }),
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let without = RunId::generate();
    insert(&repo, without, "Plan").await;

    let read = repo.get_run(id).await.expect("get").expect("the run");
    assert_eq!(read, *inserted);
    assert_eq!(
        read.definition,
        Some(RunDefinition {
            repo_url: REPO_URL.to_string(),
            commit: COMMIT.to_string(),
        })
    );
    assert_eq!(
        read.standing_rules.map(|rules| rules.source),
        Some(RulesSource::Definition)
    );
    assert_eq!(
        repo.run_definition_agents(id).await.expect("read"),
        Some(agents)
    );

    let plain = repo.get_run(without).await.expect("get").expect("the run");
    assert_eq!(plain.definition, None);
    assert_eq!(
        repo.run_definition_agents(without).await.expect("read"),
        None
    );
}

/// A Run keeps the Flow it walks, and whether it asks before its first
/// Gate, which decides whether it holds a starting place in its owner's
/// Queue; a Run kept before is counted by its Flow's name.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_keeps_its_flow_and_whether_it_asks(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let flow = serde_json::json!({ "entries": [] });
    let keep = |flow_name: &'static str, asks: Option<bool>| {
        let repo = &repo;
        let flow = &flow;
        async move {
            let id = RunId::generate();
            let inserted = repo
                .insert_run(NewRun {
                    id,
                    flow: flow_name,
                    request: "Do it",
                    user_id: &julia(),
                    task_id: None,
                    repository: None,
                    created_in_person: true,
                    control_file_patterns: &[],
                    control_files_team_id: None,
                    standing_rules: RunRules::DEFAULT,
                    automation: None,
                    definition: None,
                    kept_flow: asks.map(|asks| KeptFlow {
                        definition: flow,
                        asks,
                    }),
                })
                .await
                .expect("insert");
            assert!(matches!(inserted, RunInsert::Inserted(_)));
            id
        }
    };
    let asking = keep("notes", Some(true)).await;
    let quiet = keep("plan", Some(false)).await;
    let before = keep("plan", None).await;

    let starting = repo
        .list_starting_runs(&julia(), &["plan"], 10)
        .await
        .expect("starting");
    let mut ids: Vec<RunId> = starting.into_iter().map(|run| run.id).collect();
    ids.sort();
    let mut expected = vec![asking, before];
    expected.sort();

    assert_eq!(ids, expected);
    assert!(!ids.contains(&quiet));
    let kept = repo.run_flow(asking).await.expect("read").expect("the run");
    assert_eq!(kept.name.as_deref(), Some("notes"));
    assert_eq!(kept.definition, Some(flow.clone()));
    let older = repo.run_flow(before).await.expect("read").expect("the run");
    assert_eq!(older.definition, None);
}
