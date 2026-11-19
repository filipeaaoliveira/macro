use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::evaluation::{
    AgentChoices, EvaluationRepository, Judgment, MAX_OPEN_TRIALS, NewSuite, RunAgents, RunArm,
    SuiteInsert, TrialKey, TrialResult, TrialTokens,
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
