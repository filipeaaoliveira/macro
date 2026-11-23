use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::evaluation::{
    AgentChoices, CalibrationTally, EvaluationRepository, Judgment, MAX_OPEN_TRIALS, NewSuite,
    RunAgentRule, RunAgents, RunArm, SuiteInsert, TrialKey, TrialResult, TrialTokens,
};
use crate::domain::insights::{InsightsScope, RunInsightsRepository};
use crate::domain::models::{AgentName, Role, RunId};
use crate::outbound::pg_run_repository::PgRunRepository;

const EXPORT: &str = "00000000-0000-7000-8000-000000000401";
const IMPORT: &str = "00000000-0000-7000-8000-000000000402";
const TYPO_TRIAL: &str = "00000000-0000-7000-8000-000000000403";
const SUITE: &str = "00000000-0000-7000-8000-000000000601";
const BASE: &str = "1111111111111111111111111111111111111111";

fn run(id: &str) -> RunId {
    RunId::from_uuid(Uuid::parse_str(id).expect("valid run id"))
}

fn at(time: &str) -> DateTime<Utc> {
    time.parse().expect("valid time")
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

fn choices(pairs: &[(&str, &str)]) -> AgentChoices {
    pairs
        .iter()
        .map(|(role, agent)| {
            (
                Role::try_from(role.to_string()).expect("valid role"),
                AgentName::try_from(agent.to_string()).expect("valid agent"),
            )
        })
        .collect()
}

fn november() -> InsightsScope {
    InsightsScope {
        user_id: julia(),
        from: at("2026-11-01T00:00:00Z"),
        to: at("2026-12-01T00:00:00Z"),
        flow: None,
        cap: 1000,
    }
}

fn typo() -> TrialKey {
    TrialKey {
        suite_id: Uuid::parse_str(SUITE).expect("valid suite id"),
        task: "typo".to_string(),
    }
}

fn judgment(scorer: &str, score: f64) -> Judgment {
    Judgment {
        scorer: scorer.to_string(),
        score,
        reason: "It does what was asked.".to_string(),
        model: "anthropic/claude-haiku-5-5".to_string(),
    }
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn a_run_takes_the_agents_of_its_arms_and_a_trial_those_of_its_suite(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    assert_eq!(
        repo.run_agents(run(IMPORT))
            .await
            .expect("agents read")
            .choices,
        choices(&[("writer", "backup")])
    );
    assert_eq!(
        repo.run_agents(run(EXPORT)).await.expect("agents read"),
        RunAgents::default()
    );
    assert_eq!(
        repo.run_agents(run(TYPO_TRIAL)).await.expect("agents read"),
        RunAgents {
            choices: choices(&[("planner", "backup")]),
            ruled: AgentChoices::new(),
            trial: true,
        }
    );

    // Recording a Run's arms again keeps the first.
    repo.record_arms(
        run(IMPORT),
        &[RunArm {
            experiment: "writer".to_string(),
            arm: "control".to_string(),
            agents: AgentChoices::new(),
        }],
    )
    .await
    .expect("arms recorded");
    assert_eq!(
        repo.run_agents(run(IMPORT))
            .await
            .expect("agents read")
            .choices,
        choices(&[("writer", "backup")])
    );
}

fn agent_rule(role: &str, rule: &str, agent: &str) -> RunAgentRule {
    RunAgentRule {
        role: Role::try_from(role.to_string()).expect("valid role"),
        rule: rule.to_string(),
        agent: AgentName::try_from(agent.to_string()).expect("valid agent"),
    }
}

/// A Run keeps the Agent Rules that chose its Roles' Agents, one per Role:
/// its Steps read them beside its arms' choices, and the Run API lists
/// them.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn a_run_keeps_the_agent_rules_that_chose_its_agents(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    repo.record_agent_rules(
        run(IMPORT),
        &[
            agent_rule("planner", "docs", "backup"),
            agent_rule("reviewer", "docs-review", "primary"),
        ],
    )
    .await
    .expect("rules recorded");
    // Recording a Role's rule again keeps the first.
    repo.record_agent_rules(run(IMPORT), &[agent_rule("planner", "other", "primary")])
        .await
        .expect("rules recorded");

    let agents = repo.run_agents(run(IMPORT)).await.expect("agents read");
    assert_eq!(agents.choices, choices(&[("writer", "backup")]));
    assert_eq!(
        agents.ruled,
        choices(&[("planner", "backup"), ("reviewer", "primary")])
    );
    let kept = repo
        .runs_agent_rules(&[run(IMPORT), run(EXPORT)])
        .await
        .expect("rules read");
    assert_eq!(kept.len(), 1);
    assert_eq!(
        kept[&run(IMPORT)],
        [
            agent_rule("planner", "docs", "backup"),
            agent_rule("reviewer", "docs-review", "primary"),
        ]
    );
    assert_eq!(
        repo.run_agents(run(EXPORT)).await.expect("agents read"),
        RunAgents::default()
    );
}

/// A Run whose task's tags could not be read as it started keeps that, once,
/// and the Run API reads it beside its rules.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn a_run_keeps_that_its_task_tags_were_unread(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    repo.record_task_tags_unread(run(IMPORT))
        .await
        .expect("recorded");
    repo.record_task_tags_unread(run(IMPORT))
        .await
        .expect("recorded again");

    let unread = repo
        .runs_task_tags_unread(&[run(IMPORT), run(EXPORT)])
        .await
        .expect("read");
    assert_eq!(unread.into_iter().collect::<Vec<_>>(), [run(IMPORT)]);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn a_trials_judgments_stay_with_the_trial_and_leave_the_live_scorers_be(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    repo.record_run_score(run(EXPORT), &judgment("correctness", 0.9))
        .await
        .expect("score recorded");
    repo.record_run_score(run(IMPORT), &judgment("correctness", 0.2))
        .await
        .expect("score recorded");
    let live_before = repo
        .read_evaluation(&november())
        .await
        .expect("read")
        .scorers;

    let subject = repo
        .scoring_subject(run(TYPO_TRIAL))
        .await
        .expect("subject read")
        .expect("the trial's run is recorded");
    let trial = subject.trial.expect("the run is a trial");
    assert_eq!(trial.key, typo());
    assert_eq!(trial.benchmark, "core");
    assert!(!trial.scored);
    assert!(subject.judged.is_empty());
    assert_eq!(subject.steps.len(), 2);

    repo.record_trial_score(&typo(), &judgment("correctness", 0.1))
        .await
        .expect("trial score recorded");
    repo.record_trial_result(
        &typo(),
        &TrialResult {
            score: 1.0,
            tokens: TrialTokens {
                input: 100,
                output: 20,
                cache_read: 300,
                cache_write: 40,
            },
        },
    )
    .await
    .expect("trial result recorded");
    // A retry keeps the first result.
    repo.record_trial_result(
        &typo(),
        &TrialResult {
            score: 0.0,
            tokens: TrialTokens::default(),
        },
    )
    .await
    .expect("trial result recorded again");

    let read = repo.read_evaluation(&november()).await.expect("read");
    assert_eq!(read.scorers, live_before);
    assert_eq!(read.scorers.len(), 1);
    assert_eq!(read.scorers[0].judged, 2);
    assert_eq!(read.scorers[0].passed, 1);
    assert!((read.scorers[0].mean_score - 0.55).abs() < 1e-9);

    let suite = &read.suites[0];
    assert_eq!(suite.benchmark, "core");
    assert_eq!(suite.agents, choices(&[("planner", "backup")]));
    let trial = &suite.trials[0];
    assert_eq!(trial.task, "typo");
    assert_eq!(trial.run_id, run(TYPO_TRIAL));
    let result = trial.result.expect("the trial has its result");
    assert_eq!(result.score, 1.0);
    assert_eq!(result.tokens.input, 100);
    assert_eq!(trial.scores, vec![("correctness".to_string(), 0.1)]);

    let again = repo
        .scoring_subject(run(TYPO_TRIAL))
        .await
        .expect("subject read")
        .expect("the trial's run is recorded");
    assert!(again.trial.expect("the run is a trial").scored);
    assert_eq!(again.judged, vec!["correctness".to_string()]);
    let live = repo
        .scoring_subject(run(EXPORT))
        .await
        .expect("subject read")
        .expect("the run is recorded");
    assert_eq!(live.trial, None);
    assert_eq!(live.judged, vec!["correctness".to_string()]);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn arms_are_counted_with_their_scores_and_trials_are_left_out_of_live_insights(pool: PgPool) {
    let repo = PgRunRepository::new(pool);
    repo.record_run_score(run(IMPORT), &judgment("correctness", 0.4))
        .await
        .expect("score recorded");

    let read = repo.read_evaluation(&november()).await.expect("read");
    let arms: Vec<_> = read
        .arms
        .iter()
        .map(|arm| (arm.arm.as_str(), arm.runs, arm.ended, arm.succeeded))
        .collect();
    assert_eq!(arms, vec![("backup", 1, 1, 0), ("control", 1, 1, 1)]);
    assert_eq!(read.arms[0].agents, choices(&[("writer", "backup")]));
    assert_eq!(read.arm_scores.len(), 1);
    assert_eq!(read.arm_scores[0].arm, "backup");
    assert!((read.arm_scores[0].mean_score - 0.4).abs() < 1e-9);

    let live = repo
        .read_insights(&november())
        .await
        .expect("insights read");
    let ids: Vec<RunId> = live.runs.iter().map(|run| run.id).collect();
    assert_eq!(ids, vec![run(IMPORT), run(EXPORT)]);
}

/// A Run of Julia's of the benchmark Flow, still going.
async fn going_trial_run(pool: &PgPool, id: &str) {
    sqlx::query!(
        "INSERT INTO factory_run (id, user_id, flow, request) \
         VALUES ($1, 'macro|julia@example.com', 'benchmark', 'Fix the typo')",
        Uuid::parse_str(id).expect("valid run id"),
    )
    .execute(pool)
    .await
    .expect("run recorded");
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_suite_keeps_its_trials_runs_and_base_and_a_suite_still_running_holds_the_next(
    pool: PgPool,
) {
    let repo = PgRunRepository::new(pool.clone());
    let suite_id = Uuid::parse_str(SUITE).expect("valid suite id");
    let agents = choices(&[("writer", "backup")]);
    let owner = julia();
    let suite = |id: Uuid, trials: u16| NewSuite {
        id,
        user_id: &owner,
        benchmark: "core",
        agents: &agents,
        trials,
    };
    assert_eq!(
        repo.insert_suite(&suite(suite_id, 1)).await.expect("suite"),
        SuiteInsert::Inserted
    );
    // A suite with no trial yet is starting: another of the Benchmark waits.
    assert_eq!(
        repo.insert_suite(&suite(Uuid::from_u128(7), 1))
            .await
            .expect("suite"),
        SuiteInsert::Running
    );
    going_trial_run(&pool, TYPO_TRIAL).await;
    repo.insert_trial(&typo(), run(TYPO_TRIAL), BASE)
        .await
        .expect("trial recorded");

    assert_eq!(
        repo.run_agents(run(TYPO_TRIAL)).await.expect("agents read"),
        RunAgents {
            choices: agents.clone(),
            ruled: AgentChoices::new(),
            trial: true,
        }
    );
    assert_eq!(
        repo.trial_base_commit(run(TYPO_TRIAL))
            .await
            .expect("base read")
            .as_deref(),
        Some(BASE)
    );
    assert_eq!(
        repo.trial_base_commit(run(EXPORT))
            .await
            .expect("base read"),
        None
    );
    // Its trial still runs: another suite of the Benchmark waits, and one
    // of another Benchmark may not pass the person's open trials.
    assert_eq!(
        repo.insert_suite(&suite(Uuid::from_u128(7), 1))
            .await
            .expect("suite"),
        SuiteInsert::Running
    );
    let other = NewSuite {
        benchmark: "other",
        ..suite(Uuid::from_u128(8), MAX_OPEN_TRIALS)
    };
    assert_eq!(
        repo.insert_suite(&other).await.expect("suite"),
        SuiteInsert::TooManyTrials(1)
    );

    repo.record_branch_deleted(&typo()).await.expect("deleted");
    let trial = repo
        .trial_of(run(TYPO_TRIAL))
        .await
        .expect("trial read")
        .expect("a trial");
    assert!(trial.branch_deleted);
    assert!(!trial.scored);

    // The suite started now, so the period reaches past today. A suite
    // none of whose trials started is deleted, and never listed.
    let any_time = InsightsScope {
        from: at("2000-01-01T00:00:00Z"),
        to: at("2100-01-01T00:00:00Z"),
        ..november()
    };
    sqlx::query!(
        "UPDATE factory_run SET outcome = 'success', ended_at = now() WHERE id = $1",
        Uuid::parse_str(TYPO_TRIAL).expect("valid run id"),
    )
    .execute(&pool)
    .await
    .expect("ended");
    assert_eq!(
        repo.insert_suite(&suite(Uuid::from_u128(9), 1))
            .await
            .expect("suite"),
        SuiteInsert::Inserted
    );
    let read = repo.read_evaluation(&any_time).await.expect("read");
    assert_eq!(read.suites.len(), 1);
    assert_eq!(read.suites[0].trials[0].result, None);
    repo.delete_suite(Uuid::from_u128(9))
        .await
        .expect("deleted");
    assert_eq!(
        repo.insert_suite(&suite(Uuid::from_u128(10), 1))
            .await
            .expect("suite"),
        SuiteInsert::Inserted
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn an_arm_counts_its_runs_whose_chosen_agent_was_passed_over(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    sqlx::query!(
        "INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at, \
             session_id, action_id, offered, role, agent, passed_over, pass_reasons) \
         VALUES ($1, 1, 'write', 1, '2026-11-11 09:00Z', 'done', 'Written', '2026-11-11 09:30Z', \
             $2, $3, '{done}', 'writer', 'primary', '{backup}', '{ai_allowance_exhausted}')",
        Uuid::parse_str(IMPORT).expect("valid run id"),
        Uuid::from_u128(0x701),
        Uuid::from_u128(0x702),
    )
    .execute(&pool)
    .await
    .expect("step recorded");

    let read = repo.read_evaluation(&november()).await.expect("read");
    let fell_back: Vec<_> = read
        .arms
        .iter()
        .map(|arm| (arm.arm.as_str(), arm.fell_back))
        .collect();
    assert_eq!(fell_back, vec![("backup", 1), ("control", 0)]);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_suite_still_starting_counts_the_trials_it_plans_and_keeps_them(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let owner = julia();
    let agents = choices(&[("writer", "backup")]);
    let suite = |id: u128, benchmark: &'static str, trials: u16| NewSuite {
        id: Uuid::from_u128(id),
        user_id: &owner,
        benchmark,
        agents: &agents,
        trials,
    };

    // A suite of 15 trials has recorded none yet: a suite of another
    // Benchmark started a moment later counts them.
    assert_eq!(
        repo.insert_suite(&suite(1, "core", 15))
            .await
            .expect("suite"),
        SuiteInsert::Inserted
    );
    assert_eq!(
        repo.insert_suite(&suite(2, "other", 10))
            .await
            .expect("suite"),
        SuiteInsert::TooManyTrials(15)
    );
    assert_eq!(
        repo.insert_suite(&suite(3, "other", 5))
            .await
            .expect("suite"),
        SuiteInsert::Inserted
    );

    // A suite with a trial is never deleted.
    going_trial_run(&pool, TYPO_TRIAL).await;
    let trial = TrialKey {
        suite_id: Uuid::from_u128(1),
        task: "typo".to_string(),
    };
    repo.insert_trial(&trial, run(TYPO_TRIAL), BASE)
        .await
        .expect("trial");
    // Its recorded trial runs and its 14 others are still starting: 15,
    // with the other suite's 5, and none counted twice.
    assert_eq!(
        repo.insert_suite(&suite(4, "third", 1))
            .await
            .expect("suite"),
        SuiteInsert::TooManyTrials(20)
    );
    // A suite that stopped at a refused trial plans only those it started.
    repo.set_planned_trials(Uuid::from_u128(3), 0)
        .await
        .expect("planned");
    assert_eq!(
        repo.insert_suite(&suite(4, "third", 5))
            .await
            .expect("suite"),
        SuiteInsert::Inserted
    );
    // Once the suites started long ago, only the running trial counts.
    sqlx::query!("UPDATE factory_benchmark_suite SET created_at = now() - interval '10 minutes'")
        .execute(&pool)
        .await
        .expect("backdated");
    assert_eq!(
        repo.insert_suite(&suite(5, "fifth", 19))
            .await
            .expect("suite"),
        SuiteInsert::Inserted
    );
    assert_eq!(
        repo.insert_suite(&suite(6, "sixth", 1))
            .await
            .expect("suite"),
        SuiteInsert::TooManyTrials(20)
    );
    repo.delete_suite(Uuid::from_u128(1)).await.expect("delete");
    assert_eq!(
        repo.trial_of(run(TYPO_TRIAL))
            .await
            .expect("read")
            .map(|trial| trial.key),
        Some(trial)
    );
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn an_arm_counts_its_runs_whose_chosen_agent_was_replaced_as_fallen_back(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    // The arm chose `backup` for the writer; `primary` took the Step with
    // no Agent passed over, as when the definition no longer has `backup`.
    sqlx::query!(
        "INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at, \
             session_id, action_id, offered, role, agent) \
         VALUES ($1, 1, 'write', 1, '2026-11-11 09:00Z', 'done', 'Written', '2026-11-11 09:30Z', \
             $2, $3, '{done}', 'writer', 'primary')",
        Uuid::parse_str(IMPORT).expect("valid run id"),
        Uuid::from_u128(0x711),
        Uuid::from_u128(0x712),
    )
    .execute(&pool)
    .await
    .expect("step recorded");

    let read = repo.read_evaluation(&november()).await.expect("read");
    let fell_back: Vec<_> = read
        .arms
        .iter()
        .map(|arm| (arm.arm.as_str(), arm.fell_back))
        .collect();
    assert_eq!(fell_back, vec![("backup", 1), ("control", 0)]);
}

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn an_arm_counts_its_runs_whose_chosen_agent_was_passed_over_with_none_after(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    // The arm chose `backup` for the writer; it was passed over and no other
    // Agent was left: the Step keeps it as both its Agent and passed over.
    sqlx::query!(
        "INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at, \
             session_id, action_id, offered, role, agent, passed_over, pass_reasons) \
         VALUES ($1, 1, 'write', 1, '2026-11-11 09:00Z', 'failed', 'No quota', '2026-11-11 09:30Z', \
             $2, $3, '{done}', 'writer', 'backup', '{backup}', '{ai_allowance_exhausted}')",
        Uuid::parse_str(IMPORT).expect("valid run id"),
        Uuid::from_u128(0x721),
        Uuid::from_u128(0x722),
    )
    .execute(&pool)
    .await
    .expect("step recorded");

    let read = repo.read_evaluation(&november()).await.expect("read");
    let fell_back: Vec<_> = read
        .arms
        .iter()
        .map(|arm| (arm.arm.as_str(), arm.fell_back))
        .collect();
    assert_eq!(fell_back, vec![("backup", 1), ("control", 0)]);
}

/// A Run of an arm that ended `infrastructure` never had its work run: the
/// arm counts it among its Runs but not among those that ended, so it does
/// not count against the arm's Agents.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn an_arm_leaves_its_runs_that_ended_for_infrastructure_out_of_its_ended(pool: PgPool) {
    let lost = Uuid::parse_str("00000000-0000-7000-8000-000000000404").expect("valid run id");
    sqlx::query!(
        "INSERT INTO factory_run (id, user_id, flow, request, created_at, outcome, ended_at) \
         VALUES ($1, 'macro|julia@example.com', 'code', 'Add the search', \
                 '2026-11-13 09:00Z', 'infrastructure', '2026-11-13 09:01Z')",
        lost,
    )
    .execute(&pool)
    .await
    .expect("run recorded");
    sqlx::query!(
        "INSERT INTO factory_run_arm (run_id, experiment, arm, agents) \
         VALUES ($1, 'writer', 'backup', '{\"writer\": \"backup\"}')",
        lost,
    )
    .execute(&pool)
    .await
    .expect("arm recorded");
    let repo = PgRunRepository::new(pool);

    let read = repo.read_evaluation(&november()).await.expect("read");
    let arms: Vec<_> = read
        .arms
        .iter()
        .map(|arm| (arm.arm.as_str(), arm.runs, arm.ended, arm.succeeded))
        .collect();
    assert_eq!(arms, vec![("backup", 2, 1, 0), ("control", 1, 1, 1)]);
}

/// Insights count the Runs each Agent Rule chose an Agent for, by Role:
/// how many ended, succeeded, and fell back to another Agent of the Role.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn agent_rules_are_counted_with_the_runs_they_chose_for(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    // The export succeeded with the docs rule's writer; the import's writer
    // was passed over for lack of quota and `primary` wrote it.
    repo.record_agent_rules(run(EXPORT), &[agent_rule("writer", "docs", "docs-writer")])
        .await
        .expect("rules recorded");
    repo.record_agent_rules(run(IMPORT), &[agent_rule("writer", "docs", "docs-writer")])
        .await
        .expect("rules recorded");
    sqlx::query!(
        "INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at, \
             session_id, action_id, offered, role, agent, passed_over, pass_reasons) \
         VALUES ($1, 1, 'write', 1, '2026-11-11 09:00Z', 'done', 'Written', '2026-11-11 09:30Z', \
             $2, $3, '{done}', 'writer', 'primary', '{docs-writer}', '{ai_allowance_exhausted}')",
        Uuid::parse_str(IMPORT).expect("valid run id"),
        Uuid::from_u128(0x731),
        Uuid::from_u128(0x732),
    )
    .execute(&pool)
    .await
    .expect("step recorded");

    let read = repo.read_evaluation(&november()).await.expect("read");

    assert_eq!(read.rules.len(), 1);
    let docs = &read.rules[0];
    assert_eq!(
        (
            docs.role.as_str(),
            docs.rule.as_str(),
            docs.agent.as_str(),
            docs.runs,
            docs.ended,
            docs.succeeded,
            docs.fell_back
        ),
        ("writer", "docs", "docs-writer", 2, 2, 1, 1)
    );
}

/// A person labels a live Scorer's judgments of their Runs: a new label
/// replaces the last, a Scorer that did not judge the Run takes none, and
/// each Scorer agrees with the labels where its score passes exactly when
/// the label does, over all of the person's Runs and no one else's.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn labels_replace_the_last_and_count_the_judges_agreement(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    // An October Run of Julia's, outside the period, and one of Teo's.
    sqlx::query!(
        "INSERT INTO factory_run (id, user_id, flow, request, created_at, outcome, ended_at) VALUES \
             ($1, 'macro|julia@example.com', 'code', 'Add the search', '2026-10-05 09:00Z', \
              'success', '2026-10-05 10:00Z'), \
             ($2, 'macro|teo@example.com', 'code', 'Add the cart', '2026-11-10 09:00Z', \
              'success', '2026-11-10 10:00Z')",
        Uuid::from_u128(0x901),
        Uuid::from_u128(0x902),
    )
    .execute(&pool)
    .await
    .expect("runs recorded");
    let october = RunId::from_uuid(Uuid::from_u128(0x901));
    let teos = RunId::from_uuid(Uuid::from_u128(0x902));
    for (run_id, scorer, score) in [
        (run(EXPORT), "correctness", 0.9),
        (run(IMPORT), "correctness", 0.2),
        (october, "correctness", 0.5),
        (run(EXPORT), "tidiness", 0.3),
        (teos, "correctness", 0.9),
    ] {
        repo.record_run_score(run_id, &judgment(scorer, score))
            .await
            .expect("score recorded");
    }

    let first = repo
        .label_judgment(run(EXPORT), "correctness", false)
        .await
        .expect("labelled")
        .expect("the scorer judged the run");
    assert!(!first.passed);
    let replaced = repo
        .label_judgment(run(EXPORT), "correctness", true)
        .await
        .expect("labelled")
        .expect("the scorer judged the run");
    assert!(replaced.passed);
    assert!(replaced.labelled_at >= first.labelled_at);
    let unjudged = repo
        .label_judgment(run(IMPORT), "tidiness", true)
        .await
        .expect("labelled");
    assert_eq!(unjudged, None);
    // The import's judge failed it and Julia agrees; the October Run passed
    // at exactly the pass score and she says it should have failed; the
    // tidiness judge failed the export and she says it passed.
    for (run_id, scorer, passed) in [
        (run(IMPORT), "correctness", false),
        (october, "correctness", false),
        (run(EXPORT), "tidiness", true),
        (teos, "correctness", true),
    ] {
        repo.label_judgment(run_id, scorer, passed)
            .await
            .expect("labelled")
            .expect("the scorer judged the run");
    }

    let judgments = repo.run_judgments(run(EXPORT)).await.expect("read");
    let labels: Vec<_> = judgments
        .iter()
        .map(|judgment| {
            (
                judgment.scorer.as_str(),
                judgment.score,
                judgment.label.map(|label| label.passed),
            )
        })
        .collect();
    assert_eq!(
        labels,
        vec![
            ("correctness", 0.9, Some(true)),
            ("tidiness", 0.3, Some(true))
        ]
    );
    let teos_judgments = repo.run_judgments(teos).await.expect("read");
    assert_eq!(
        teos_judgments[0].label.map(|label| label.passed),
        Some(true)
    );

    let expected = vec![
        CalibrationTally {
            scorer: "correctness".to_string(),
            labelled: 3,
            agreed: 2,
        },
        CalibrationTally {
            scorer: "tidiness".to_string(),
            labelled: 1,
            agreed: 0,
        },
    ];
    assert_eq!(repo.calibrations(&julia()).await.expect("read"), expected);
    let read = repo.read_evaluation(&november()).await.expect("read");
    assert_eq!(read.calibrations, expected);
}

/// Each trial reads when its Run started and ended, for the time it took.
#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people", "evaluation"))
)]
async fn a_trial_reads_when_its_run_started_and_ended(pool: PgPool) {
    let repo = PgRunRepository::new(pool);

    let read = repo.read_evaluation(&november()).await.expect("read");

    let trial = &read.suites[0].trials[0];
    assert_eq!(trial.started_at, at("2026-11-12T09:00:00Z"));
    assert_eq!(trial.ended_at, Some(at("2026-11-12T09:30:00Z")));
}
