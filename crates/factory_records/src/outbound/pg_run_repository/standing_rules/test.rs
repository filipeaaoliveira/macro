//! Teams' and Projects' Standing Rules, the rules a Run keeps, and a Rule
//! Answer to a Gate.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::test::{insert, julia, key, outcome};
use crate::domain::models::{NewRun, RunId, RunInsert};
use crate::domain::ports::RunRepository;
use crate::domain::standing_rules::{
    OrchestratorDefinition, ProjectRules, ProjectRulesSet, RulesSource, RunRules, StandInKinds,
    StandingRule, StandingRulesRepository,
};
use crate::outbound::pg_run_repository::PgRunRepository;

const TEAM: &str = "00000000-0000-7000-8000-000000000201";
const PROJECT: &str = "00000000-0000-7000-8000-000000000301";

fn id(id: &str) -> Uuid {
    Uuid::parse_str(id).expect("valid id")
}

const PLAN_AND_MERGE: OrchestratorDefinition = OrchestratorDefinition {
    plan: true,
    blockers: false,
    merge: true,
    stand_in: StandInKinds {
        plan: false,
        blockers: true,
    },
};

/// A team's rules and its Control Files share its settings: setting or
/// clearing either leaves the other as it was, and clearing both removes
/// the team's settings.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team"))
)]
async fn a_team_s_rules_and_control_files_are_set_apart(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let team = id(TEAM);
    assert_eq!(repo.team_rules(team).await.expect("read"), None);

    let patterns = vec!["ci/**".to_string()];
    repo.set_team_control_files(team, Some(&patterns))
        .await
        .expect("set patterns");
    repo.set_team_rules(team, Some(PLAN_AND_MERGE))
        .await
        .expect("set rules");
    assert_eq!(
        repo.team_rules(team).await.expect("read"),
        Some(PLAN_AND_MERGE)
    );

    repo.set_team_control_files(team, None)
        .await
        .expect("clear patterns");
    assert_eq!(repo.team_control_files(team).await.expect("read"), None);
    assert_eq!(
        repo.team_rules(team).await.expect("read"),
        Some(PLAN_AND_MERGE)
    );

    // Rules that decide nothing are the team's own all the same.
    repo.set_team_rules(team, Some(OrchestratorDefinition::default()))
        .await
        .expect("set none");
    assert_eq!(
        repo.team_rules(team).await.expect("read"),
        Some(OrchestratorDefinition::default())
    );

    repo.set_team_rules(team, None).await.expect("clear rules");
    assert_eq!(repo.team_rules(team).await.expect("read"), None);
    let rows = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM factory_team_settings"#)
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(rows, 0);
}

/// A Project's rules are kept per team, listed, replaced and removed; a
/// Project no initiative has is refused.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team", "project"))
)]
async fn a_project_s_rules_are_kept_for_its_team(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let (team, project) = (id(TEAM), id(PROJECT));
    assert_eq!(repo.project_rules(team, project).await.expect("read"), None);

    let set = repo
        .set_project_rules(team, project, Some(PLAN_AND_MERGE))
        .await
        .expect("set");
    assert_eq!(set, ProjectRulesSet::Set);
    assert_eq!(
        repo.project_rules(team, project).await.expect("read"),
        Some(PLAN_AND_MERGE)
    );
    assert_eq!(
        repo.project_rules(Uuid::now_v7(), project)
            .await
            .expect("read"),
        None
    );

    let merge_only = OrchestratorDefinition {
        merge: true,
        ..OrchestratorDefinition::default()
    };
    repo.set_project_rules(team, project, Some(merge_only))
        .await
        .expect("replace");
    assert_eq!(
        repo.list_project_rules(team).await.expect("list"),
        [ProjectRules {
            project_id: project,
            definition: merge_only,
        }]
    );

    let missing = repo
        .set_project_rules(team, Uuid::now_v7(), Some(merge_only))
        .await
        .expect("set");
    assert_eq!(missing, ProjectRulesSet::NoSuchProject);

    repo.set_project_rules(team, project, None)
        .await
        .expect("remove");
    assert!(
        repo.list_project_rules(team)
            .await
            .expect("list")
            .is_empty()
    );
}

/// A Run keeps the rules it was created with and where they came from; a
/// Run created before Runs kept rules reads with none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_keeps_the_rules_it_was_created_with(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let rules = RunRules {
        definition: PLAN_AND_MERGE,
        source: RulesSource::Project,
        project_id: Some(id(PROJECT)),
    };
    let run_id = RunId::generate();
    let RunInsert::Inserted(inserted) = repo
        .insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Fix it",
            user_id: &julia(),
            task_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    assert_eq!(inserted.standing_rules, Some(rules));
    let read = repo.get_run(run_id).await.expect("get").expect("kept");
    assert_eq!(read.standing_rules, Some(rules));

    let before = insert(&repo, RunId::generate(), "Older").await;
    sqlx::query!(
        "UPDATE factory_run SET standing_rules = NULL, standing_rules_source = NULL WHERE id = $1",
        before.id.as_uuid(),
    )
    .execute(&pool)
    .await
    .expect("forget");
    let read = repo.get_run(before.id).await.expect("get").expect("kept");
    assert_eq!(read.standing_rules, None);
}

/// A Rule Answer names its rule, has no author, and, like a person's, does
/// not replace an answer given first.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_rule_answers_a_gate_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    let run_id = RunId::generate();
    insert(&repo, run_id, "Fix it").await;
    for seq in [1, 2] {
        repo.start_step(key(run_id, seq), &super::super::test::entry("plan"), 1)
            .await
            .expect("start");
        repo.open_gate(
            key(run_id, seq),
            "Approve?",
            &[outcome("approve"), outcome("revise")],
        )
        .await
        .expect("open");
    }

    let kept = repo
        .answer_gate_by_rule(
            key(run_id, 1),
            &outcome("approve"),
            StandingRule::ApprovePlan,
        )
        .await
        .expect("rule answer")
        .expect("the gate is recorded");
    assert_eq!(kept.rule, Some(StandingRule::ApprovePlan));
    repo.answer_gate(key(run_id, 1), &outcome("revise"), None, &julia(), None)
        .await
        .expect("a person's answer after it");
    repo.answer_gate(key(run_id, 2), &outcome("revise"), None, &julia(), None)
        .await
        .expect("a person's answer");
    // The person's answer stays, and is what the rule reads back.
    let kept = repo
        .answer_gate_by_rule(
            key(run_id, 2),
            &outcome("approve"),
            StandingRule::ApprovePlan,
        )
        .await
        .expect("a rule answer after it")
        .expect("the gate is recorded");
    assert_eq!(kept.answer, outcome("revise"));
    assert_eq!(kept.rule, None);
    let none = repo
        .answer_gate_by_rule(
            key(run_id, 9),
            &outcome("approve"),
            StandingRule::ApprovePlan,
        )
        .await
        .expect("no gate");
    assert_eq!(none, None);

    let steps = repo.list_steps(run_id).await.expect("steps");
    let ruled = steps[0].gate.as_ref().and_then(|gate| gate.answer.as_ref());
    let ruled = ruled.expect("answered");
    assert_eq!(ruled.answer, outcome("approve"));
    assert_eq!(ruled.rule, Some(StandingRule::ApprovePlan));
    assert_eq!(ruled.user_id, None);
    let person = steps[1].gate.as_ref().and_then(|gate| gate.answer.as_ref());
    let person = person.expect("answered");
    assert_eq!(person.answer, outcome("revise"));
    assert_eq!(person.rule, None);
    assert_eq!(person.user_id, Some(julia()));
}
