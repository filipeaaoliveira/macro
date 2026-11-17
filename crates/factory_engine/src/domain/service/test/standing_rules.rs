//! A Gate the Run's Standing Rules answer, or leave to a person, as the
//! worker asks when the Gate opens.

use chrono::Utc;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    ControlFileChanges, EntryId, NewRun, Outcome, Repository, RunId, RunInsert, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{
    GateKind, OrchestratorDefinition, RulesSource, RunRules, StandingRule,
};

use super::{FakeExecutor, julia};
use crate::domain::fakes::FakeRepo;
use crate::domain::gate::GateRuling;
use crate::domain::ports::StepService;
use crate::domain::service::StepServiceImpl;
use crate::domain::standing_rules::RuleAnswer;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

const EVERY_RULE: OrchestratorDefinition = OrchestratorDefinition {
    plan: true,
    blockers: true,
    merge: true,
};

fn base() -> Repository {
    Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    }
}

/// A `code` Run of Julia's with `rules`, at its merge Gate on Step 1.
async fn run_with(repo: &FakeRepo, rules: RunRules) -> StepKey {
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Fix the login bug",
            user_id: &julia(),
            task_id: None,
            repository: Some(&base()),
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
    let key = StepKey {
        run_id: run.id,
        seq: 1,
    };
    let entry = EntryId::try_from("merge-gate".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.open_gate(key, "Merge?", &[outcome("approve"), outcome("revise")])
        .await
        .expect("open");
    key
}

fn team_rules(definition: OrchestratorDefinition) -> RunRules {
    RunRules {
        definition,
        source: RulesSource::Team,
        project_id: None,
    }
}

fn merge_ruling(key: StepKey) -> GateRuling {
    GateRuling {
        key,
        kind: GateKind::Merge,
        fix_rounds: 0,
        answers: vec![outcome("approve"), outcome("revise")],
    }
}

fn steps(repo: &FakeRepo) -> StepServiceImpl<FakeRepo, FakeExecutor> {
    StepServiceImpl::new(repo.clone(), FakeExecutor::default())
}

fn approved_by_rule() -> Option<RuleAnswer> {
    Some(RuleAnswer {
        rule: StandingRule::MergeWhenGreen,
        answer: outcome("approve"),
    })
}

/// A Gate the Run's rules decide is answered at once, and the answer is
/// recorded with the rule that gave it; asking again returns it.
#[tokio::test]
async fn a_rule_answers_its_gate_and_the_answer_names_it() {
    let repo = FakeRepo::default();
    let key = run_with(&repo, team_rules(EVERY_RULE)).await;

    let ruled = steps(&repo)
        .rule_gate(merge_ruling(key))
        .await
        .expect("rule");
    assert_eq!(ruled, approved_by_rule());
    assert_eq!(
        *repo.rule_answers.lock().expect("answers"),
        [(key, outcome("approve"), StandingRule::MergeWhenGreen)]
    );

    // A retry finds the answer recorded, even once a hold would stop the
    // rule now.
    repo.hold_main(&base(), "abc", "- test (failure)", Utc::now())
        .await
        .expect("hold");
    let again = steps(&repo)
        .rule_gate(merge_ruling(key))
        .await
        .expect("rule");
    assert_eq!(again, approved_by_rule());
    assert_eq!(repo.rule_answers.lock().expect("answers").len(), 1);
}

/// A Gate no rule decides is left for a person, and nothing is recorded.
#[tokio::test]
async fn a_gate_no_rule_decides_waits_for_a_person() {
    let repo = FakeRepo::default();
    let key = run_with(&repo, team_rules(OrchestratorDefinition::default())).await;

    let ruled = steps(&repo)
        .rule_gate(merge_ruling(key))
        .await
        .expect("rule");

    assert_eq!(ruled, None);
    assert!(repo.rule_answers.lock().expect("answers").is_empty());
}

/// A Tampered Run's merge Gate, and a merge Gate while the base branch is
/// held, stay a person's whatever the rules say.
#[tokio::test]
async fn no_rule_approves_tampered_files_or_merges_through_a_hold() {
    let repo = FakeRepo::default();
    let tampered = run_with(&repo, team_rules(EVERY_RULE)).await;
    repo.set_control_file_changes(
        tampered.run_id,
        Some(&ControlFileChanges {
            tampered: vec![".github/workflows/ci.yml".to_string()],
            named: Vec::new(),
            unlisted: false,
        }),
    )
    .await
    .expect("keep");
    let ruled = steps(&repo)
        .rule_gate(merge_ruling(tampered))
        .await
        .expect("rule");
    assert_eq!(ruled, None);

    let held = run_with(&repo, team_rules(EVERY_RULE)).await;
    repo.hold_main(&base(), "abc", "- test (failure)", Utc::now())
        .await
        .expect("hold");
    let ruled = steps(&repo)
        .rule_gate(merge_ruling(held))
        .await
        .expect("rule");
    assert_eq!(ruled, None);
    assert!(repo.rule_answers.lock().expect("answers").is_empty());
}

/// A Run created before Runs kept rules has none: its Gates, opened after
/// rules existed, are still a person's.
#[tokio::test]
async fn a_run_created_before_rules_existed_waits_for_a_person() {
    let repo = FakeRepo::default();
    let key = run_with(&repo, team_rules(EVERY_RULE)).await;
    if let Some(run) = repo.runs.lock().expect("runs").get_mut(&key.run_id) {
        run.standing_rules = None;
    }

    let ruled = steps(&repo)
        .rule_gate(merge_ruling(key))
        .await
        .expect("rule");

    assert_eq!(ruled, None);
}

/// A review panel's Blockers go to a fix twice in a row on their own; the
/// Blockers found after those wait for a person.
#[tokio::test]
async fn blockers_after_two_rule_fixes_wait_for_a_person() {
    let repo = FakeRepo::default();
    let ruling = |key, fix_rounds| GateRuling {
        key,
        kind: GateKind::Blockers,
        fix_rounds,
        answers: vec![outcome("fix"), outcome("stop")],
    };

    let second = run_with(&repo, RunRules::DEFAULT).await;
    let ruled = steps(&repo)
        .rule_gate(ruling(second, 1))
        .await
        .expect("rule");
    assert_eq!(
        ruled,
        Some(RuleAnswer {
            rule: StandingRule::FixBlockers,
            answer: outcome("fix"),
        })
    );

    let third = run_with(&repo, RunRules::DEFAULT).await;
    let ruled = steps(&repo)
        .rule_gate(ruling(third, 2))
        .await
        .expect("rule");
    assert_eq!(ruled, None);
}
