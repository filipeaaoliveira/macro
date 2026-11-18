//! A Gate the Run's Standing Rules answer, or leave to a person, as the
//! worker asks when the Gate opens.

use chrono::Utc;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    ControlFileChanges, EntryId, NewRun, Outcome, Repository, RunId, RunInsert, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RuleGateKind, RulesSource, RunRules, StandingRule,
};

use super::{FakeExecutor, julia};
use crate::domain::fakes::FakeRepo;
use crate::domain::gate::GateRuling;
use crate::domain::ports::StepService;
use crate::domain::service::StepServiceImpl;
use crate::domain::standing_rules::{MAX_RULE_FIX_ROUNDS, RuleAnswer};

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

const EVERY_RULE: OrchestratorDefinition = OrchestratorDefinition {
    stand_in: factory_records::domain::standing_rules::StandInKinds {
        plan: false,
        blockers: false,
    },
    plan: true,
    blockers: true,
    merge: true,
    owner_gates: false,
    task_thread: false,
    queue_limit: None,
    orchestrator: factory_records::domain::standing_rules::OrchestratorKinds {
        plan: false,
        blockers: false,
    },
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
            automation: None,
            definition: None,
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
        kind: RuleGateKind::Merge,
        fix_rounds: 0,
        fix_limit: 0,
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
        kind: RuleGateKind::Blockers,
        fix_rounds,
        fix_limit: MAX_RULE_FIX_ROUNDS,
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

/// A fix approves no Control Files: a Tampered Run's Blockers still go to
/// one, while its merge waits for a person.
#[tokio::test]
async fn a_tampered_run_s_blockers_go_to_a_fix_and_its_merge_to_a_person() {
    let repo = FakeRepo::default();
    let key = run_with(&repo, team_rules(EVERY_RULE)).await;
    repo.set_control_file_changes(
        key.run_id,
        Some(&ControlFileChanges {
            tampered: vec!["package.json".to_string()],
            named: Vec::new(),
            unlisted: false,
        }),
    )
    .await
    .expect("keep");
    let blockers = GateRuling {
        key,
        kind: RuleGateKind::Blockers,
        fix_rounds: 0,
        fix_limit: MAX_RULE_FIX_ROUNDS,
        answers: vec![outcome("fix"), outcome("stop")],
    };

    assert_eq!(
        steps(&repo).rule_gate(blockers).await.expect("rule"),
        Some(RuleAnswer {
            rule: StandingRule::FixBlockers,
            answer: outcome("fix"),
        })
    );
    let merge = run_with(&repo, team_rules(EVERY_RULE)).await;
    repo.set_control_file_changes(
        merge.run_id,
        Some(&ControlFileChanges {
            tampered: Vec::new(),
            named: Vec::new(),
            unlisted: true,
        }),
    )
    .await
    .expect("keep");
    assert_eq!(
        steps(&repo)
            .rule_gate(merge_ruling(merge))
            .await
            .expect("rule"),
        None
    );
}

/// The Gate keeps its first answer, read back after the rule's write: a
/// Gate a person answered first is never taken as ruled.
#[tokio::test]
async fn a_gate_a_person_answered_first_is_not_taken_as_ruled() {
    let repo = FakeRepo::default();
    let key = run_with(&repo, team_rules(EVERY_RULE)).await;
    // The person's answer lands between the rule's read and its write.
    repo.steps
        .lock()
        .expect("steps")
        .iter_mut()
        .filter(|(run, step)| *run == key.run_id && step.seq == key.seq)
        .for_each(|(_, step)| {
            if let Some(gate) = step.gate.as_mut() {
                gate.answer = Some(factory_records::domain::models::GateAnswer {
                    reply_confidence: None,
                    by_stand_in: false,
                    agent_session_id: None,
                    answer: outcome("revise"),
                    note: None,
                    user_id: Some(julia()),
                    rule: None,
                    approved_control_files: None,
                    answered_at: Utc::now(),
                });
            }
        });
    let kept = repo
        .answer_gate_by_rule(key, &outcome("approve"), StandingRule::MergeWhenGreen)
        .await
        .expect("write")
        .expect("recorded");
    assert_eq!(kept.answer, outcome("revise"));

    // Read through the service, the Gate is a person's, not a rule's.
    let ruled = steps(&repo)
        .rule_gate(merge_ruling(key))
        .await
        .expect("rule");
    assert_eq!(ruled, None);
}

/// A question Gate an agent opened is never answered by a rule, even by a
/// ruling that names a kind whose answers it offers: only the Run's owner
/// answers an agent.
#[tokio::test]
async fn a_rule_never_answers_an_agents_question() {
    let repo = FakeRepo::default();
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
            standing_rules: team_rules(EVERY_RULE),
            automation: None,
            definition: None,
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
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.end_step(key, &Outcome::question(), "Approve or revise?")
        .await
        .expect("end");
    repo.open_gate(
        key,
        "Approve or revise?",
        &[outcome("approve"), outcome("revise")],
    )
    .await
    .expect("open");

    let ruled = steps(&repo)
        .rule_gate(GateRuling {
            key,
            kind: RuleGateKind::Plan,
            fix_rounds: 0,
            fix_limit: 0,
            answers: vec![outcome("approve"), outcome("revise")],
        })
        .await
        .expect("rule");

    assert_eq!(ruled, None);
    assert!(repo.rule_answers.lock().expect("answers").is_empty());
}
