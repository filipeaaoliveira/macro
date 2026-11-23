//! Teams' and Projects' Standing Rules, the rules a Run keeps, and a Rule
//! Answer to a Gate.

use macro_db_migrator::MACRO_DB_MIGRATIONS;
use sqlx::PgPool;
use uuid::Uuid;

use super::super::test::{insert, julia, key, outcome};
use crate::domain::models::{BudgetStop, NewRun, RunId, RunInsert};
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
    owner_gates: false,
    quiet_gates: false,
    task_thread: false,
    rerun_flaky_ci: false,
    queue_limit: None,
    token_budget: None,
    orchestrator: crate::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
    stand_in: StandInKinds {
        plan: false,
        blockers: true,
    },
};

const OWNER_GATES_ON: OrchestratorDefinition = OrchestratorDefinition {
    plan: false,
    blockers: true,
    merge: false,
    owner_gates: true,
    quiet_gates: false,
    task_thread: false,
    rerun_flaky_ci: false,
    queue_limit: None,
    token_budget: None,
    orchestrator: crate::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
    stand_in: StandInKinds {
        plan: false,
        blockers: false,
    },
};

const EVERY_SETTING: OrchestratorDefinition = OrchestratorDefinition {
    plan: true,
    blockers: true,
    merge: true,
    owner_gates: true,
    quiet_gates: true,
    task_thread: true,
    rerun_flaky_ci: false,
    queue_limit: None,
    token_budget: None,
    orchestrator: crate::domain::standing_rules::OrchestratorKinds {
        plan: true,
        blockers: true,
    },
    stand_in: StandInKinds {
        plan: true,
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
            project_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
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

/// The owner section is kept with the rules it sits beside: a team's, a
/// Project's, and those a Run is created with.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team", "project"))
)]
async fn the_owner_section_is_kept_with_the_rules(pool: PgPool) {
    kept_everywhere(pool, OWNER_GATES_ON).await;
}

/// A definition with every setting on, the owner section, quiet Gates, the
/// stand-in and the task thread among them, is kept as a team's, a Project's and a Run's.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team", "project"))
)]
async fn every_setting_is_kept_together(pool: PgPool) {
    kept_everywhere(pool, EVERY_SETTING).await;
}

/// The Queue limit is kept with a team's and a Project's rules, replaced
/// with them, and goes with the team's when they go back to the default.
/// A Run does not keep it.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team", "project"))
)]
async fn the_queue_limit_is_kept_with_the_rules(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let (team, project) = (id(TEAM), id(PROJECT));
    let limited = OrchestratorDefinition {
        queue_limit: Some(3),
        token_budget: None,
        ..PLAN_AND_MERGE
    };

    repo.set_team_rules(team, Some(limited))
        .await
        .expect("set the team's");
    assert_eq!(repo.team_rules(team).await.expect("read"), Some(limited));
    repo.set_project_rules(team, project, Some(limited))
        .await
        .expect("set the project's");
    assert_eq!(
        repo.project_rules(team, project).await.expect("read"),
        Some(limited)
    );
    assert_eq!(
        repo.list_project_rules(team).await.expect("list"),
        [ProjectRules {
            project_id: project,
            definition: limited,
        }]
    );

    // Rules saved again without a limit set none.
    repo.set_project_rules(team, project, Some(PLAN_AND_MERGE))
        .await
        .expect("replace the project's");
    assert_eq!(
        repo.project_rules(team, project).await.expect("read"),
        Some(PLAN_AND_MERGE)
    );

    // Control Files keep the team's row once its rules go back to the
    // default: the limit goes with them.
    let patterns = vec!["ci/**".to_string()];
    repo.set_team_control_files(team, Some(&patterns))
        .await
        .expect("set patterns");
    repo.set_team_rules(team, None).await.expect("clear rules");
    assert_eq!(repo.team_rules(team).await.expect("read"), None);
    let limit = sqlx::query_scalar!(
        "SELECT queue_limit FROM factory_team_settings WHERE team_id = $1",
        team,
    )
    .fetch_one(&pool)
    .await
    .expect("read the row");
    assert_eq!(limit, None);

    let rules = RunRules {
        definition: limited,
        source: RulesSource::Team,
        project_id: None,
    };
    let run_id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Fix it",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let read = repo.get_run(run_id).await.expect("get").expect("kept");
    let kept = read.standing_rules.expect("rules");
    assert_eq!(kept.definition, PLAN_AND_MERGE);
}

/// The migration that adds `quiet_gates` sets it on no kept definition:
/// a team's, a Project's and a Run's, each with the owner section off as
/// before it, keep notifying the Run's owner once it runs.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team", "project"))
)]
async fn the_quiet_gates_migration_keeps_saved_definitions_notifying(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let (team, project) = (id(TEAM), id(PROJECT));
    const { assert!(!PLAN_AND_MERGE.owner_gates && !PLAN_AND_MERGE.quiet_gates) };
    repo.set_team_rules(team, Some(PLAN_AND_MERGE))
        .await
        .expect("set the team's");
    repo.set_project_rules(team, project, Some(PLAN_AND_MERGE))
        .await
        .expect("set the project's");
    let rules = RunRules {
        definition: PLAN_AND_MERGE,
        source: RulesSource::Team,
        project_id: None,
    };
    let run_id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Fix it",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };

    // The rows now stand as they did before the migration; run it on them.
    let mut tx = pool.begin().await.expect("begin");
    sqlx::raw_sql(include_str!(
        "../../../../../macro_db_client/migrations/20261119010000_factory_quiet_gates.sql"
    ))
    .execute(&mut *tx)
    .await
    .expect("migrate");
    tx.commit().await.expect("commit");

    assert_eq!(
        repo.team_rules(team).await.expect("read"),
        Some(PLAN_AND_MERGE)
    );
    assert_eq!(
        repo.project_rules(team, project).await.expect("read"),
        Some(PLAN_AND_MERGE)
    );
    let read = repo.get_run(run_id).await.expect("get").expect("kept");
    assert_eq!(read.standing_rules, Some(rules));
    let quiet = sqlx::query_scalar!(
        r#"SELECT
            (SELECT count(*) FROM factory_team_settings WHERE 'quiet_gates' = ANY(standing_rules))
            + (SELECT count(*) FROM factory_project_rules WHERE 'quiet_gates' = ANY(standing_rules))
            + (SELECT count(*) FROM factory_run WHERE 'quiet_gates' = ANY(standing_rules))
            AS "count!""#
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(quiet, 0);
}

/// Keep `definition` as a team's, a Project's and a new Run's rules, and
/// read each back the same.
async fn kept_everywhere(pool: PgPool, definition: OrchestratorDefinition) {
    let repo = PgRunRepository::new(pool);
    let (team, project) = (id(TEAM), id(PROJECT));
    repo.set_team_rules(team, Some(definition))
        .await
        .expect("set the team's");
    assert_eq!(repo.team_rules(team).await.expect("read"), Some(definition));
    repo.set_project_rules(team, project, Some(definition))
        .await
        .expect("set the project's");
    assert_eq!(
        repo.project_rules(team, project).await.expect("read"),
        Some(definition)
    );

    let rules = RunRules {
        definition,
        source: RulesSource::Team,
        project_id: None,
    };
    let run_id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Fix it",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let read = repo.get_run(run_id).await.expect("get").expect("kept");
    assert_eq!(read.standing_rules, Some(rules));
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
    repo.answer_gate(
        key(run_id, 1),
        &outcome("revise"),
        None,
        &julia(),
        None,
        None,
    )
    .await
    .expect("a person's answer after it");
    repo.answer_gate(
        key(run_id, 2),
        &outcome("revise"),
        None,
        &julia(),
        None,
        None,
    )
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

/// Keep a new Run of Julia's with no task under `rules`.
async fn insert_under(repo: &PgRunRepository, rules: RunRules) -> RunId {
    let run_id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Fix it",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: true,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    run_id
}

/// The token budget is kept with a team's and a Project's rules, and a Run
/// keeps it with the rules it started with, whatever they say later.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "team", "project"))
)]
async fn the_token_budget_is_kept_with_the_rules_and_the_run(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let (team, project) = (id(TEAM), id(PROJECT));
    let budgeted = OrchestratorDefinition {
        token_budget: Some(2_000_000),
        ..PLAN_AND_MERGE
    };

    repo.set_team_rules(team, Some(budgeted))
        .await
        .expect("set the team's");
    assert_eq!(repo.team_rules(team).await.expect("read"), Some(budgeted));
    repo.set_project_rules(team, project, Some(budgeted))
        .await
        .expect("set the project's");
    assert_eq!(
        repo.project_rules(team, project).await.expect("read"),
        Some(budgeted)
    );
    assert_eq!(
        repo.list_project_rules(team).await.expect("list")[0].definition,
        budgeted
    );

    let run_id = insert_under(
        &repo,
        RunRules {
            definition: budgeted,
            source: RulesSource::Team,
            project_id: None,
        },
    )
    .await;

    // Rules saved again without a budget set none, and the Run keeps its.
    repo.set_team_rules(team, Some(PLAN_AND_MERGE))
        .await
        .expect("replace the team's");
    assert_eq!(
        repo.team_rules(team).await.expect("read"),
        Some(PLAN_AND_MERGE)
    );
    let read = repo.get_run(run_id).await.expect("get").expect("kept");
    let kept = read.standing_rules.expect("rules");
    assert_eq!(kept.definition, budgeted);
    assert_eq!(read.budget_stop, None);
}

/// A Run its budget ends keeps why, once: a retry keeps the first stop. A
/// Run with no budget keeps none.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_budget_stop_is_kept_once_and_only_with_a_budget(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let budgeted = RunRules {
        definition: OrchestratorDefinition {
            token_budget: Some(50_000),
            ..PLAN_AND_MERGE
        },
        source: RulesSource::Team,
        project_id: None,
    };
    let spent = insert_under(&repo, budgeted).await;
    let unread = insert_under(&repo, budgeted).await;
    let unbudgeted = insert_under(&repo, RunRules::DEFAULT).await;

    repo.record_budget_stop(spent, BudgetStop::Spent { used: 61_234 })
        .await
        .expect("record");
    repo.record_budget_stop(spent, BudgetStop::Unread)
        .await
        .expect("record again");
    repo.record_budget_stop(unread, BudgetStop::Unread)
        .await
        .expect("record");
    repo.record_budget_stop(unbudgeted, BudgetStop::Spent { used: 1 })
        .await
        .expect("record without a budget");

    let read = |id| {
        let repo = repo.clone();
        async move { repo.get_run(id).await.expect("get").expect("kept") }
    };
    assert_eq!(
        read(spent).await.budget_stop,
        Some(BudgetStop::Spent { used: 61_234 })
    );
    assert_eq!(read(unread).await.budget_stop, Some(BudgetStop::Unread));
    assert_eq!(read(unbudgeted).await.budget_stop, None);
}

/// A Run's Step sessions are read each once, a session two Steps shared
/// (one went on from the other's question) among them, without the Steps
/// that worked in none, nor another Run's.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_run_s_step_sessions_are_read_each_once(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let run_id = RunId::generate();
    insert(&repo, run_id, "Plan it").await;
    let other = RunId::generate();
    insert(&repo, other, "Plan another").await;
    let (shared, own, others) = (
        Uuid::from_u128(0x801),
        Uuid::from_u128(0x802),
        Uuid::from_u128(0x803),
    );
    for (run, seq, session, action) in [
        (run_id, 1, Some(shared), 0x901),
        (run_id, 2, Some(shared), 0x902),
        (run_id, 3, None, 0),
        (run_id, 4, Some(own), 0x904),
        (other, 1, Some(others), 0x905),
    ] {
        sqlx::query!(
            "INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, \
                 ended_at, session_id, action_id, offered) \
             VALUES ($1, $2, 'plan', 1, '2026-11-11 09:00Z', 'done', 'Planned', \
                 '2026-11-11 09:30Z', $3, $4, CASE WHEN $3::uuid IS NULL THEN NULL ELSE '{done}'::text[] END)",
            run.as_uuid(),
            seq,
            session,
            session.map(|_| Uuid::from_u128(action)),
        )
        .execute(&pool)
        .await
        .expect("step recorded");
    }

    let mut sessions = repo.step_sessions(run_id).await.expect("read");
    sessions.sort();
    assert_eq!(sessions, [shared, own]);
    assert_eq!(
        repo.step_sessions(RunId::generate()).await.expect("read"),
        Vec::<Uuid>::new()
    );
}
