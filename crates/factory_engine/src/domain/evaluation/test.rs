use std::sync::Arc;

use ai_usage::domain::session_usage::SessionTokens;
use chrono::Utc;
use factory_records::domain::evaluation::{EvaluationRepository, NewSuite, TrialKey, TrialTokens};
use factory_records::domain::models::{
    AgentName, EntryId, NewRun, Outcome, Role, RunEnd, RunId, RunInsert, RunOutcome, Step, StepEnd,
    StepKey, StepSession, StepTerminal,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::RunRules;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use uuid::Uuid;

use super::service::{ScoringServiceImpl, trial_score};
use super::{EvaluationDefinition, EvaluationDefinitionError};
use crate::domain::agents::AgentDefinition;
use crate::domain::fakes::{FakeJudge, FakeRepo, FakeSession};
use crate::domain::insights::fakes::FakeUsage;
use crate::domain::ports::ScoringService;

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid user id")
}

fn role(name: &str) -> Role {
    Role::try_from(name.to_string()).expect("valid role")
}

fn agent(name: &str) -> AgentName {
    AgentName::try_from(name.to_string()).expect("valid agent")
}

fn read(definition: serde_json::Value) -> Result<EvaluationDefinition, String> {
    serde_json::from_value(definition).map_err(|error| error.to_string())
}

fn agents() -> AgentDefinition {
    serde_json::from_value(json!({
        "agents": {
            "primary": { "bot": "00000000-0000-0000-0000-00000000c1a0" },
            "backup": { "bot": "00000000-0000-0000-0000-00000000c0de" }
        },
        "roles": { "writer": ["primary", "backup"], "planner": ["primary"] }
    }))
    .expect("an agent definition")
}

/// A definition with a correctness Scorer on every Run, a sampled one on
/// half of `code`'s, an Experiment on `code`'s writer, and a Benchmark
/// whose trials the correctness Scorer judges.
fn definition() -> EvaluationDefinition {
    read(json!({
        "scorers": [
            {
                "name": "correctness",
                "instructions": "Does the change do what the request asks?",
                "samplePercent": 100
            },
            {
                "name": "tidiness",
                "instructions": "Is the change small and tidy?",
                "flows": ["code"],
                "samplePercent": 0
            }
        ],
        "experiments": [{
            "name": "writer-agent",
            "flow": "code",
            "arms": [
                { "name": "control" },
                { "name": "backup", "weight": 3, "agents": { "writer": "backup" } }
            ]
        }],
        "benchmarks": [{
            "name": "core",
            "repository": "https://github.com/acme/widgets",
            "tasks": [
                { "name": "typo", "request": "Fix the typo in the README." },
                { "name": "export", "request": "Add a CSV export." }
            ],
            "scorers": ["correctness"]
        }]
    }))
    .expect("a definition")
}

#[test]
fn a_definition_is_checked_as_a_whole() {
    let checked = |value| read(value).expect_err("refused");

    assert!(
        checked(json!({ "scorers": [
            { "name": "a", "instructions": "x", "samplePercent": 101 }
        ]}))
        .contains("sample from 0 to 100")
    );
    assert!(
        checked(json!({ "scorers": [
            { "name": "a", "instructions": " ", "samplePercent": 10 }
        ]}))
        .contains("instructions")
    );
    assert!(
        checked(json!({ "experiments": [
            { "name": "e", "flow": "code", "arms": [{ "name": "only" }] }
        ]}))
        .contains("at least two arms")
    );
    assert!(
        checked(json!({ "experiments": [
            { "name": "e", "flow": "benchmark", "arms": [{ "name": "a" }, { "name": "b" }] }
        ]}))
        .contains("not a flow")
    );
    assert!(
        checked(json!({ "experiments": [
            { "name": "one", "flow": "code", "arms": [
                { "name": "a" }, { "name": "b", "agents": { "writer": "backup" } }
            ]},
            { "name": "two", "flow": "code", "arms": [
                { "name": "a" }, { "name": "b", "agents": { "writer": "primary" } }
            ]}
        ]}))
        .contains("both choose the agent of role writer")
    );
    assert!(
        checked(json!({ "benchmarks": [
            { "name": "b", "repository": "https://github.com/acme/widgets", "tasks": [] }
        ]}))
        .contains("must have tasks")
    );
    assert!(
        checked(json!({ "benchmarks": [{
            "name": "b",
            "repository": "https://github.com/acme/widgets",
            "tasks": [{ "name": "t", "request": "Do it." }],
            "scorers": ["missing"]
        }]}))
        .contains("names scorer missing")
    );

    // Arms may name only the agent definition's Agents.
    let unknown = read(json!({ "experiments": [
        { "name": "e", "flow": "code", "arms": [
            { "name": "a" }, { "name": "b", "agents": { "writer": "nobody" } }
        ]}
    ]}))
    .expect("readable");
    assert!(matches!(
        EvaluationDefinition::configured(Some(unknown.clone()), Some(&agents())),
        Err(EvaluationDefinitionError::UnknownAgent { .. })
    ));
    assert!(EvaluationDefinition::configured(Some(definition()), Some(&agents())).is_ok());
    assert_eq!(
        EvaluationDefinition::configured(None, None),
        Ok(EvaluationDefinition::default())
    );
}

#[test]
fn a_task_keeps_its_arm_and_the_weights_split_the_tasks() {
    let definition = definition();
    let task = Uuid::from_u128(42);
    let first = definition.assign_arms("code", Some(task));
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].experiment, "writer-agent");
    for _ in 0..5 {
        assert_eq!(definition.assign_arms("code", Some(task)), first);
    }

    // No task, or another Flow, takes no arm.
    assert!(definition.assign_arms("code", None).is_empty());
    assert!(definition.assign_arms("plan", Some(task)).is_empty());

    // A weight of 3 to 1 gives the backup arm about three tasks in four.
    let backup = (0..1000u128)
        .filter(|task| {
            definition.assign_arms("code", Some(Uuid::from_u128(*task)))[0].arm == "backup"
        })
        .count();
    assert!((650..850).contains(&backup), "backup took {backup} of 1000");
    let backup_arm = definition.assign_arms(
        "code",
        Some(Uuid::from_u128(
            (0..1000u128)
                .find(|task| {
                    definition.assign_arms("code", Some(Uuid::from_u128(*task)))[0].arm == "backup"
                })
                .expect("a backup task"),
        )),
    );
    assert_eq!(
        backup_arm[0].agents.get(&role("writer")),
        Some(&agent("backup"))
    );
}

#[test]
fn a_scorer_samples_its_share_of_its_flows_runs_the_same_way_each_time() {
    let definition = read(json!({ "scorers": [
        { "name": "half", "instructions": "x", "flows": ["code"], "samplePercent": 50 },
        { "name": "none", "instructions": "x", "samplePercent": 0 },
        { "name": "all", "instructions": "x", "samplePercent": 100 }
    ]}))
    .expect("a definition");
    let [half, none, all] = definition.scorers() else {
        panic!("three scorers");
    };
    let runs: Vec<RunId> = (0..1000u128)
        .map(|run| RunId::from_uuid(Uuid::from_u128(run)))
        .collect();

    let sampled = runs
        .iter()
        .filter(|run| half.samples(Some("code"), **run))
        .count();
    assert!((400..600).contains(&sampled), "sampled {sampled} of 1000");
    assert!(
        runs.iter()
            .all(|run| half.samples(Some("code"), *run) == half.samples(Some("code"), *run))
    );
    assert!(!runs.iter().any(|run| half.samples(Some("plan"), *run)));
    assert!(!runs.iter().any(|run| none.samples(Some("code"), *run)));
    assert!(runs.iter().all(|run| all.samples(None, *run)));
}

/// A Run of Julia's of `flow`, ended `outcome`, whose write Step ended
/// `written`, in the agent session `session`.
async fn ended_run(
    repo: &FakeRepo,
    flow: &str,
    outcome: RunOutcome,
    written: &str,
    session: Uuid,
) -> RunId {
    let id = RunId::generate();
    let inserted = repo
        .insert_run(NewRun {
            id,
            flow,
            request: "Fix the typo in the README.",
            user_id: &julia(),
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
        .await
        .expect("insert");
    assert!(matches!(inserted, RunInsert::Inserted(_)));
    repo.steps.lock().expect("steps").push((
        id,
        Step {
            seq: 1,
            entry: EntryId::try_from("write".to_string()).expect("valid"),
            visit: 1,
            started_at: Utc::now(),
            end: Some(StepEnd {
                outcome: Outcome::try_from(written.to_string()).expect("valid"),
                output: "Wrote the fix.".to_string(),
                ended_at: Utc::now(),
            }),
            gate: None,
            role: None,
            agent: None,
            passed_over: Vec::new(),
            checks: Vec::new(),
            terminal: StepTerminal::default(),
        },
    ));
    repo.sessions.lock().expect("sessions").push(FakeSession {
        key: StepKey { run_id: id, seq: 1 },
        session: StepSession {
            session_id: session,
            action_id: Uuid::from_u128(9),
            agent: None,
            bot: None,
        },
        offered: Vec::new(),
        handed: None,
        turn: None,
        turn_place: None,
        released: false,
        prepared_at: None,
        opened: true,
    });
    repo.runs
        .lock()
        .expect("runs")
        .get_mut(&id)
        .expect("kept")
        .end = Some(RunEnd {
        outcome,
        ended_at: Utc::now(),
    });
    id
}

fn usage(session: Uuid) -> FakeUsage {
    FakeUsage::new(vec![SessionTokens {
        session_id: session,
        input: 1200,
        output: 300,
        cache_read: 5000,
        cache_write: 700,
    }])
}

#[tokio::test]
async fn a_sampled_run_is_judged_once_by_each_scorer_that_takes_it() {
    let repo = FakeRepo::default();
    let session = Uuid::from_u128(7);
    let run = ended_run(&repo, "code", RunOutcome::Success, "done", session).await;
    let judge = Arc::new(FakeJudge::default());
    judge.then(0.9);
    let scoring = ScoringServiceImpl::new(
        repo.clone(),
        judge.clone(),
        usage(session),
        Arc::new(definition()),
    );

    scoring.score_run(run).await.expect("scored");
    scoring.score_run(run).await.expect("scored again");

    // `tidiness` samples none of `code`'s Runs; `correctness` judged once.
    let kept = repo.evaluation.lock().expect("evaluation").clone();
    let scores = &kept.run_scores[&run];
    assert_eq!(scores.len(), 1);
    assert_eq!(scores[0].scorer, "correctness");
    assert_eq!(scores[0].score, 0.9);
    assert_eq!(scores[0].model, "fake/judge");
    let asked = judge.asked();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].0, julia());
    assert!(
        asked[0]
            .1
            .instructions
            .contains("Does the change do what the request asks?")
    );
    let prompt: serde_json::Value =
        serde_json::from_str(&asked[0].1.prompt).expect("the run as JSON");
    assert_eq!(prompt["request"], "Fix the typo in the README.");
    assert_eq!(prompt["steps"][0]["outcome"], "done");
    assert!(kept.trial_results.is_empty());
}

#[tokio::test]
async fn a_failed_judge_is_asked_again_and_a_cancelled_run_is_not_judged() {
    let repo = FakeRepo::default();
    let session = Uuid::from_u128(7);
    let run = ended_run(&repo, "code", RunOutcome::Failed, "red", session).await;
    let judge = Arc::new(FakeJudge::default());
    judge.then_fail();
    let scoring = ScoringServiceImpl::new(
        repo.clone(),
        judge.clone(),
        usage(session),
        Arc::new(definition()),
    );

    scoring.score_run(run).await.expect_err("the judge failed");
    assert!(
        repo.evaluation
            .lock()
            .expect("evaluation")
            .run_scores
            .is_empty()
    );
    scoring.score_run(run).await.expect("scored on the retry");
    assert_eq!(
        repo.evaluation.lock().expect("evaluation").run_scores[&run].len(),
        1
    );

    let cancelled = ended_run(&repo, "code", RunOutcome::Cancelled, "done", session).await;
    scoring
        .score_run(cancelled)
        .await
        .expect("nothing to judge");
    assert!(
        !repo
            .evaluation
            .lock()
            .expect("evaluation")
            .run_scores
            .contains_key(&cancelled)
    );
    assert_eq!(judge.asked().len(), 2);
}

#[tokio::test]
async fn a_trial_keeps_its_score_tokens_and_judgments_apart_from_the_live_scorers() {
    let repo = FakeRepo::default();
    let session = Uuid::from_u128(7);
    let live = ended_run(&repo, "code", RunOutcome::Success, "done", session).await;
    let trial_run = ended_run(&repo, "benchmark", RunOutcome::Manual, "red", session).await;
    let suite_id = Uuid::from_u128(600);
    let trial = TrialKey {
        suite_id,
        task: "typo".to_string(),
    };
    let configuration = [(role("writer"), agent("backup"))].into_iter().collect();
    repo.insert_suite(&NewSuite {
        id: suite_id,
        user_id: &julia(),
        benchmark: "core",
        agents: &configuration,
    })
    .await
    .expect("suite");
    repo.insert_trial(&trial, trial_run).await.expect("trial");
    let judge = Arc::new(FakeJudge::default());
    judge.then(0.9);
    judge.then(0.2);
    let scoring = ScoringServiceImpl::new(
        repo.clone(),
        judge.clone(),
        usage(session),
        Arc::new(definition()),
    );

    scoring.score_run(live).await.expect("live run scored");
    let live_scores = repo
        .evaluation
        .lock()
        .expect("evaluation")
        .run_scores
        .clone();
    scoring.score_run(trial_run).await.expect("trial scored");
    scoring
        .score_run(trial_run)
        .await
        .expect("trial scored again");

    let kept = repo.evaluation.lock().expect("evaluation").clone();
    // The live Scorers' judgments did not move.
    assert_eq!(kept.run_scores, live_scores);
    assert!(!kept.run_scores.contains_key(&trial_run));
    // The trial keeps its mechanical score, its tokens and its judgment.
    let result = kept.trial_results[&(suite_id, "typo".to_string())];
    assert_eq!(result.score, 0.5);
    assert_eq!(
        result.tokens,
        TrialTokens {
            input: 1200,
            output: 300,
            cache_read: 5000,
            cache_write: 700,
        }
    );
    let judged = &kept.trial_scores[&(suite_id, "typo".to_string())];
    assert_eq!(judged.len(), 1);
    assert_eq!(judged[0].scorer, "correctness");
    assert_eq!(judged[0].score, 0.2);
    assert_eq!(judge.asked().len(), 2);
}

#[tokio::test]
async fn a_trial_is_scored_by_how_its_write_step_ended() {
    let repo = FakeRepo::default();
    let session = Uuid::from_u128(7);
    let cases = [
        (RunOutcome::Success, "done", 1.0),
        (RunOutcome::Manual, "red", 0.5),
        (RunOutcome::Manual, "unchanged", 0.0),
        (RunOutcome::Failed, "failed", 0.0),
        (RunOutcome::Cancelled, "done", 0.0),
    ];
    for (outcome, written, score) in cases {
        let run = ended_run(&repo, "benchmark", outcome, written, session).await;
        let subject = repo
            .scoring_subject(run)
            .await
            .expect("read")
            .expect("kept");
        assert_eq!(trial_score(&subject), score, "{outcome:?} after {written}");
    }
}
