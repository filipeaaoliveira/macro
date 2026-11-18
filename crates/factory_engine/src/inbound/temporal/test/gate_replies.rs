//! Gate replies and the stand-in, on a running Temporal: a person's words
//! answer a Gate or are asked back, and the stand-in's answer stands once
//! its window passes, unless the owner overrides it.

use std::time::Duration;

use factory_records::domain::models::{ControlFileChanges, RunOutcome};
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RulesSource, RunRules, StandInKinds,
};

use super::{CodeAgent, CodeWalk, julia, walk_code_with};
use crate::domain::fakes::{FakeGateModel, FakeGithub, FakeRepo, ci};
use crate::domain::gate_replies::GateReplySettings;

/// Rules that fix Blockers, as the default does, and stand in for the
/// owner at the plan Gate.
fn standing_in_at_the_plan() -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
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

/// Words the model reads as `approve`, sure enough, answer the plan Gate:
/// the Run goes on, and the Gate keeps the words and how sure it was.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_reply_answers_the_plan_gate_and_the_run_goes_on() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let mut plan_gate = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            gates: Some((model, GateReplySettings::default())),
            ..CodeWalk::default()
        },
        |run| {
            let plan_gate = &mut plan_gate;
            async move {
                let plan = run.next_gate().await;
                run.reply(plan, "Looks right, go ahead.").await;
                let merge = run.next_gate().await;
                *plan_gate = Some(run.gate(plan).await);
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let answer = plan_gate.and_then(|gate| gate.answer).expect("answered");
    assert_eq!(answer.answer.as_str(), "approve");
    assert_eq!(answer.note.as_deref(), Some("Looks right, go ahead."));
    assert_eq!(answer.reply_confidence, Some(95));
    assert_eq!(answer.user_id, Some(julia()));
}

/// Words the model cannot read are asked back: the Gate waits on, and a
/// button answer then goes as any other.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn an_unclear_reply_is_asked_back_and_the_gate_waits() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let model = FakeGateModel::picking(None, 0.9);
    let mut plan_gate = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            gates: Some((model, GateReplySettings::default())),
            ..CodeWalk::default()
        },
        |run| {
            let plan_gate = &mut plan_gate;
            async move {
                let plan = run.next_gate().await;
                run.reply(plan, "Hmm, what about the tests?").await;
                // Once asked back, the Gate takes an answer again.
                run.answer(plan, "approve").await;
                let merge = run.next_gate().await;
                *plan_gate = Some(run.gate(plan).await);
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let gate = plan_gate.expect("read");
    assert_eq!(
        gate.asked_back.map(|asked| asked.reply).as_deref(),
        Some("Hmm, what about the tests?")
    );
    let answer = gate.answer.expect("answered");
    assert_eq!(answer.answer.as_str(), "approve");
    assert_eq!(answer.reply_confidence, None);
}

/// The stand-in answers the plan Gate for the owner, and once its window
/// passes the Run goes on with its answer.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn the_stand_in_s_answer_stands_once_its_window_passes() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let mut plan_gate = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: standing_in_at_the_plan(),
            gates: Some((model, window(1))),
            ..CodeWalk::default()
        },
        |run| {
            let plan_gate = &mut plan_gate;
            async move {
                let plan = run.next_gate().await;
                let merge = run.next_gate().await;
                *plan_gate = Some(run.gate(plan).await);
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let gate = plan_gate.expect("read");
    let answer = gate.answer.expect("answered");
    assert!(answer.by_stand_in);
    assert_eq!(answer.answer.as_str(), "approve");
    assert_eq!(answer.user_id, None);
    assert_eq!(gate.stand_in.map(|stood| stood.confidence), Some(95));
}

/// An owner who answers within the window overrides the stand-in: their
/// answer is the Gate's, and the Run goes on at once, long before the
/// window would end. Sent back, the plan is asked again; the stand-in,
/// whose model fails this time, leaves it to the owner.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn an_override_in_the_window_replaces_the_stand_in_s_answer() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let mut plan_gate = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: standing_in_at_the_plan(),
            gates: Some((model, window(3_600))),
            ..CodeWalk::default()
        },
        |run| {
            let plan_gate = &mut plan_gate;
            async move {
                let plan = run.next_gate().await;
                // Taken once the stand-in answered and the window opened.
                run.answer_with(plan, "revise", Some("Keep it smaller"))
                    .await;
                let again = run.next_gate().await;
                *plan_gate = Some(run.gate(plan).await);
                run.answer(again, "approve").await;
                let merge = run.next_gate().await;
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let gate = plan_gate.expect("read");
    assert!(gate.stand_in.is_some());
    let answer = gate.answer.expect("answered");
    assert_eq!(answer.answer.as_str(), "revise");
    assert!(!answer.by_stand_in);
    assert_eq!(answer.user_id, Some(julia()));
    assert_eq!(answer.note.as_deref(), Some("Keep it smaller"));
}

/// A Run that turns Tampered while the stand-in's window runs gives the
/// Gate back to its owner when the window ends: the stand-in's answer is
/// taken back, and the owner's answer is the Gate's.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_run_tampered_in_the_window_waits_for_its_owner() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let model = FakeGateModel::picking(Some("approve"), 0.95);
    let mut plan_gate = None;

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: standing_in_at_the_plan(),
            gates: Some((model, window(3))),
            ..CodeWalk::default()
        },
        |run| {
            let plan_gate = &mut plan_gate;
            async move {
                let plan = run.next_gate().await;
                // Once the stand-in answered, the branch turns Tampered.
                while run.gate(plan).await.stand_in.is_none() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
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
                // Refused while the window runs, taken once the Gate is
                // the owner's again.
                tokio::time::sleep(Duration::from_secs(4)).await;
                run.answer(plan, "approve").await;
                let merge = run.next_gate().await;
                *plan_gate = Some(run.gate(plan).await);
                run.answer(merge, "approve").await;
            }
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let gate = plan_gate.expect("read");
    assert_eq!(gate.stand_in, None);
    let answer = gate.answer.expect("answered");
    assert!(!answer.by_stand_in);
    assert_eq!(answer.user_id, Some(julia()));
}
