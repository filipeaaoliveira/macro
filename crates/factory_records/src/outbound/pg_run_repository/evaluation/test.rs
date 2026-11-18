use chrono::{DateTime, Utc};
use macro_db_migrator::MACRO_DB_MIGRATIONS;
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::evaluation::{
    AgentChoices, EvaluationRepository, Judgment, NewSuite, RunArm, TrialKey, TrialResult,
    TrialTokens,
};
use crate::domain::insights::{InsightsScope, RunInsightsRepository};
use crate::domain::models::{AgentName, Role, RunId};
use crate::outbound::pg_run_repository::PgRunRepository;

const EXPORT: &str = "00000000-0000-7000-8000-000000000401";
const IMPORT: &str = "00000000-0000-7000-8000-000000000402";
const TYPO_TRIAL: &str = "00000000-0000-7000-8000-000000000403";
const SUITE: &str = "00000000-0000-7000-8000-000000000601";

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
        repo.run_agents(run(IMPORT)).await.expect("agents read"),
        choices(&[("writer", "backup")])
    );
    assert_eq!(
        repo.run_agents(run(EXPORT)).await.expect("agents read"),
        AgentChoices::new()
    );
    assert_eq!(
        repo.run_agents(run(TYPO_TRIAL)).await.expect("agents read"),
        choices(&[("planner", "backup")])
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
        repo.run_agents(run(IMPORT)).await.expect("agents read"),
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

#[sqlx::test(
    migrator = "MACRO_DB_MIGRATIONS",
    fixtures(path = "../fixtures", scripts("people"))
)]
async fn a_suite_keeps_its_trials_runs(pool: PgPool) {
    let repo = PgRunRepository::new(pool.clone());
    let suite_id = Uuid::parse_str(SUITE).expect("valid suite id");
    let agents = choices(&[("writer", "backup")]);
    repo.insert_suite(&NewSuite {
        id: suite_id,
        user_id: &julia(),
        benchmark: "core",
        agents: &agents,
    })
    .await
    .expect("suite recorded");
    sqlx::query(
        "INSERT INTO factory_run (id, user_id, flow, request) \
         VALUES ($1, 'macro|julia@example.com', 'benchmark', 'Fix the typo')",
    )
    .bind(Uuid::parse_str(TYPO_TRIAL).expect("valid run id"))
    .execute(&pool)
    .await
    .expect("run recorded");
    repo.insert_trial(&typo(), run(TYPO_TRIAL))
        .await
        .expect("trial recorded");

    assert_eq!(
        repo.run_agents(run(TYPO_TRIAL)).await.expect("agents read"),
        agents
    );
    // The suite started now, so the period reaches past today.
    let any_time = InsightsScope {
        from: at("2000-01-01T00:00:00Z"),
        to: at("2100-01-01T00:00:00Z"),
        ..november()
    };
    let read = repo.read_evaluation(&any_time).await.expect("read");
    assert_eq!(read.suites.len(), 1);
    assert_eq!(read.suites[0].trials[0].result, None);
}
