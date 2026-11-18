//! The owner section on a running Temporal: the Gates a person answers are
//! told to the Run's owner, and none with the section off; a Gate the
//! stand-in answers is told to no one, unless its answer is taken back.

use std::time::Duration;

use factory_records::domain::models::{ControlFileChanges, RunOutcome};
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RulesSource, RunRules, StandInKinds,
};

use super::{CodeAgent, CodeWalk, PATIENCE, walk_code_with};
use crate::domain::fakes::{FakeGateModel, FakeGithub, FakeNotifier, FakeRepo, ci};
use crate::domain::gate_owner::notice_id;
use crate::domain::gate_replies::GateReplySettings;

fn rules(owner_gates: bool) -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            owner_gates,
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// Walk a `code` Run whose review finds Blockers once, which the rule
/// sends to a fix, and whose owner approves the plan and the merge; return
/// the places of the Gates its owner was told of.
async fn told_of(owner_gates: bool) -> Vec<u16> {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    *agent.reviews.lock().expect("reviews") = ["changes", "pass", "pass"].to_vec();
    let notifier = FakeNotifier::default();

    let ended = walk_code_with(
        &agent,
        &github,
        &FakeRepo::default(),
        CodeWalk {
            rules: rules(owner_gates),
            notifier: Some(notifier.clone()),
            ..CodeWalk::default()
        },
        |run| async move {
            run.answer_next_gate("approve").await;
            run.answer_next_gate("approve").await;
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let sent = notifier.sent.lock().expect("sent");
    sent.iter().map(|notice| notice.metadata.seq).collect()
}

/// With the owner section on, the plan and merge Gates, which wait for a
/// person, are each told to the owner once; the Blockers Gate the rule
/// answered is not.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn the_gates_a_person_answers_are_told_to_the_owner() {
    let told = told_of(true).await;
    assert_eq!(told.len(), 2, "{told:?}");
    assert_eq!(told[0], 1, "the plan's Gate");
    assert!(told[1] > told[0], "{told:?}");
}

/// With the owner section off, the Run walks as before and no Gate is
/// told to anyone.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn with_the_owner_section_off_no_gate_is_told() {
    assert_eq!(told_of(false).await, Vec::<u16>::new());
}

/// Rules that fix Blockers, with the owner section on and the stand-in
/// answering the plan Gate.
fn owner_and_stand_in() -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            owner_gates: true,
            task_thread: false,
            stand_in: StandInKinds {
                plan: true,
                blockers: false,
            },
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// The stand-in's window set to `seconds`.
fn window(seconds: u64) -> GateReplySettings {
    GateReplySettings {
        stand_in_window: Duration::from_secs(seconds),
        ..GateReplySettings::default()
    }
}

/// A plan Gate the stand-in answers, once its window passes, tells the
/// owner nothing: only the merge Gate, which waits for them, is told.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_gate_the_stand_in_answers_is_not_told_to_the_owner() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let mut merge_gate = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: owner_and_stand_in(),
            notifier: Some(notifier.clone()),
            gates: Some((model, window(1))),
            ..CodeWalk::default()
        },
        |run| {
            let merge_gate = &mut merge_gate;
            async move {
                let plan = run.next_gate().await;
                let merge = run.next_gate().await;
                assert!(run.gate(plan).await.answer.expect("answered").by_stand_in);
                *merge_gate = Some(merge);
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let told: Vec<u16> = notifier
        .sent
        .lock()
        .expect("sent")
        .iter()
        .map(|notice| notice.metadata.seq)
        .collect();
    assert_eq!(told, [merge_gate.expect("the merge gate opened")]);
}

/// A stand-in's answer taken back when the window ends (the branch turned
/// Tampered in it) leaves the Gate to the owner, who is told of it then;
/// their answer marks the notification done.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_gate_whose_stand_in_answer_is_taken_back_is_told_to_the_owner() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let mut run_id = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: owner_and_stand_in(),
            notifier: Some(notifier.clone()),
            gates: Some((model, window(3))),
            ..CodeWalk::default()
        },
        |run| {
            let run_id = &mut run_id;
            let notifier = notifier.clone();
            async move {
                *run_id = Some(run.run_id);
                let plan = run.next_gate().await;
                while run.gate(plan).await.stand_in.is_none() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                // Nothing is told while the stand-in's answer may stand.
                assert!(notifier.sent.lock().expect("sent").is_empty());
                run.repo
                    .control_file_changes
                    .lock()
                    .expect("changes")
                    .insert(
                        run.run_id,
                        ControlFileChanges {
                            tampered: vec!["AGENTS.md".to_string()],
                            named: Vec::new(),
                            unlisted: false,
                        },
                    );
                tokio::time::sleep(Duration::from_secs(4)).await;
                run.answer(plan, "approve").await;
                let merge = run.next_gate().await;
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let run_id = run_id.expect("walked");
    let told: Vec<u16> = notifier
        .sent
        .lock()
        .expect("sent")
        .iter()
        .map(|notice| notice.metadata.seq)
        .collect();
    assert_eq!(
        told.iter().filter(|seq| **seq == 1).count(),
        1,
        "the plan's Gate, told once: {told:?}"
    );
    assert!(
        notifier
            .done
            .lock()
            .expect("done")
            .contains(&notice_id(run_id, 1))
    );
}

/// Words that answer a Gate the owner was told of mark its notification
/// done, as a button's answer does.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_reply_that_answers_a_told_gate_marks_its_notification_done() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let mut run_id = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: rules(true),
            notifier: Some(notifier.clone()),
            gates: Some((model, GateReplySettings::default())),
            ..CodeWalk::default()
        },
        |run| {
            let run_id = &mut run_id;
            async move {
                *run_id = Some(run.run_id);
                let plan = run.next_gate().await;
                run.reply(plan, "Looks right, go ahead.").await;
                let merge = run.next_gate().await;
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let run_id = run_id.expect("walked");
    assert_eq!(
        notifier
            .sent
            .lock()
            .expect("sent")
            .first()
            .map(|notice| notice.metadata.seq),
        Some(1)
    );
    assert!(
        notifier
            .done
            .lock()
            .expect("done")
            .contains(&notice_id(run_id, 1))
    );
}

/// The places of the Gates `notifier` told the owner of, in order.
fn told(notifier: &FakeNotifier) -> Vec<u16> {
    notifier
        .sent
        .lock()
        .expect("sent")
        .iter()
        .map(|notice| notice.metadata.seq)
        .collect()
}

/// Words the owner gives in the stand-in's window, which the factory asks
/// back on, leave the Gate waiting for them: they are told of it then,
/// once, and not while the stand-in's answer could stand.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn words_asked_back_in_the_stand_in_s_window_tell_the_owner() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    // The stand-in approves; the owner's words then read as unclear.
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    model.then(None, 0.9);

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: owner_and_stand_in(),
            notifier: Some(notifier.clone()),
            gates: Some((model, window(3_600))),
            ..CodeWalk::default()
        },
        |run| {
            let notifier = notifier.clone();
            async move {
                let plan = run.next_gate().await;
                while run.gate(plan).await.stand_in.is_none() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                assert!(notifier.sent.lock().expect("sent").is_empty());
                run.reply(plan, "Hmm, what about the tests?").await;
                tokio::time::timeout(PATIENCE, async {
                    while told(&notifier).is_empty() {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .expect("the owner is told");
                assert!(run.gate(plan).await.asked_back.is_some());
                run.answer(plan, "approve").await;
                let merge = run.next_gate().await;
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let told = told(&notifier);
    assert_eq!(told.iter().filter(|seq| **seq == 1).count(), 1, "{told:?}");
}

/// Words asked back on a Gate the owner was already told of tell them
/// nothing more: the Gate's notification stays the one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn words_asked_back_on_a_told_gate_do_not_tell_the_owner_again() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    let model = FakeGateModel::picking(None, 0.9);

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: rules(true),
            notifier: Some(notifier.clone()),
            gates: Some((model, GateReplySettings::default())),
            ..CodeWalk::default()
        },
        |run| {
            let notifier = notifier.clone();
            async move {
                let plan = run.next_gate().await;
                tokio::time::timeout(PATIENCE, async {
                    while told(&notifier).is_empty() {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .expect("the owner is told");
                run.reply(plan, "Hmm, what about the tests?").await;
                while run.gate(plan).await.asked_back.is_none() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                run.answer(plan, "approve").await;
                let merge = run.next_gate().await;
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let told = told(&notifier);
    assert_eq!(told.iter().filter(|seq| **seq == 1).count(), 1, "{told:?}");
}
