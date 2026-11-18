//! The owner section on a running Temporal: the Gates a person answers are
//! told to the Run's owner, and none with the section off.

use factory_records::domain::models::RunOutcome;
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules};

use super::{CodeAgent, CodeWalk, walk_code_with};
use crate::domain::fakes::{FakeGithub, FakeNotifier, FakeRepo, ci};

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
