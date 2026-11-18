//! A notify-only Gate and the task thread on a running Temporal: a `code`
//! Run whose plan Gate only tells its owner goes on to write without an
//! answer, and posts its updates to one thread on its task.
//!
//! With `FACTORY_RECORD_HISTORY` set to a folder, the Run's history is
//! written there as `code_notify_only_plan_with_thread.json`, for
//! `replay.rs` to replay.

use factory_records::domain::models::{EntryId, Outcome, RunOutcome};
use factory_records::domain::standing_rules::{OrchestratorDefinition, RulesSource, RunRules};
use temporalio_client::WorkflowFetchHistoryOptions;

use super::{CodeAgent, CodeWalk, client, walk_code_with};
use crate::domain::built_in_flows::BuiltInFlow;
use crate::domain::fakes::{FakeGithub, FakeNotifier, FakeRepo, FakeTaskThread, ci};
use crate::domain::flow::{Flow, GateDefinition, StepUse, Target, Transition};
use crate::domain::gate_owner::FACTORY_SIGNATURE;
use crate::temporal::FlowWorkflowDefinition;

/// The built-in `code` Flow, whose plan Gate only tells the Run's owner:
/// the plan's `done` goes on to `write`.
fn code_with_a_notify_only_plan() -> Flow {
    let mut flow = BuiltInFlow::named("code")
        .expect("built in")
        .read(|_| Some(StepUse::Work))
        .expect("valid flow");
    let plan = &mut flow.entries[0];
    plan.gate = Some(GateDefinition {
        question: "The plan is ready; the run goes on to write it.".to_string(),
        answers: Vec::new(),
        after: None,
        kind: None,
        person_only: false,
        notify_only: true,
    });
    let write = EntryId::try_from("write".to_string()).expect("valid");
    plan.on.insert(
        Outcome::done(),
        vec![Transition::to(Target::Entry { entry: write })],
    );
    flow
}

/// Rules that fix Blockers, with the owner section and the task thread on.
fn told_and_posted() -> RunRules {
    RunRules {
        definition: OrchestratorDefinition {
            owner_gates: true,
            task_thread: true,
            ..OrchestratorDefinition::DEFAULT
        },
        source: RulesSource::Team,
        project_id: None,
    }
}

/// The plan's notify-only Gate tells the owner, without a push, and the
/// Run goes on to write; only the merge Gate waits for them. Every update
/// is posted once to the Run's thread on its task.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_notify_only_plan_gate_tells_the_owner_and_the_run_goes_on() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let notifier = FakeNotifier::default();
    let thread = FakeTaskThread::default();

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        CodeWalk {
            rules: told_and_posted(),
            flow: Some(code_with_a_notify_only_plan()),
            notifier: Some(notifier.clone()),
            thread: Some(thread.clone()),
            ..CodeWalk::default()
        },
        |run| async move {
            let merge = run.next_gate().await;
            assert!(merge > 1, "the plan's gate waits for no one");
            run.answer(merge, "approve").await;
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let run_id = *repo
        .runs
        .lock()
        .expect("runs")
        .keys()
        .next()
        .expect("the run");

    let sent = notifier.sent.lock().expect("sent").clone();
    assert_eq!(sent.len(), 2, "{sent:?}");
    assert_eq!(sent[0].metadata.seq, 1);
    assert!(!sent[0].push);
    assert!(sent[0].metadata.notify_only);
    assert!(!sent[1].metadata.notify_only);
    assert!(sent[1].push, "the merge gate waits on the owner");

    let texts = thread.texts();
    assert!(texts.iter().all(|text| text.ends_with(FACTORY_SIGNATURE)));
    assert!(texts[0].starts_with(
        "**A factory run started.** [Open the run](https://macro.test/app/factory/runs/"
    ));
    let has = |start: &str| texts.iter().filter(|text| text.starts_with(start)).count();
    assert_eq!(has("Step 1, `plan`"), 1, "{texts:#?}");
    assert_eq!(has("Review round 1 ended `pass`"), 1, "{texts:#?}");
    assert!(
        !texts.iter().any(|text| text.contains(", `review`")),
        "a reviewer's Step is told with its round: {texts:#?}"
    );
    assert_eq!(
        has("Step 1 tells the run's owner, and the run goes on."),
        1,
        "{texts:#?}"
    );
    assert_eq!(has("**The factory run ended** `success`."), 1, "{texts:#?}");
    let merge_gate = sent[1].metadata.seq;
    assert_eq!(
        has(&format!(
            "Step {merge_gate}'s gate was answered `approve` by the run's owner."
        )),
        1,
        "{texts:#?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("ended `published`.\n\nPull request #")),
        "{texts:#?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("ended `merged`.\n\nPull request #")),
        "{texts:#?}"
    );
    assert_eq!(
        texts
            .last()
            .map(|text| text.starts_with("**The factory run ended**")),
        Some(true)
    );

    if let Some(folder) = macro_env_var::maybe_read_env("FACTORY_RECORD_HISTORY") {
        let history = client()
            .await
            .get_workflow_handle::<FlowWorkflowDefinition>(run_id.to_string())
            .fetch_history(WorkflowFetchHistoryOptions::default())
            .to_json()
            .await
            .expect("the history reads");
        std::fs::write(
            format!("{folder}/code_notify_only_plan_with_thread.json"),
            history,
        )
        .expect("the history is written");
    }
}
