//! The `update` Step: the base merged in by GitHub, or a conflict the
//! writer resolves or hands to a person.

use factory_records::domain::merge_queue::{BranchUpdate, MergeQueueRepository};
use factory_records::domain::models::Repository;

use factory_records::domain::models::EntryId;
use factory_records::domain::ports::RunRepository;

use super::{FakeAgent, input, step};
use crate::domain::built_in_steps::{BuiltInStepError, BuiltInSteps};
use crate::domain::fakes::{FakeGithub, FakeRepo, FakeWatches, ci};
use crate::domain::models::BranchUpdating;
use crate::domain::models::{StepInput, StepReport};
use crate::domain::ports::StepExecutor;
use crate::domain::pull_request_steps::PullRequestSteps;

const NEW_BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const UPDATED: &str = "cccccccccccccccccccccccccccccccccccccccc";
const RESOLVED: &str = "dddddddddddddddddddddddddddddddddddddddd";

/// GitHub with the Run's pull request open at [`FakeGithub::HEAD`], on a
/// base that moved to [`NEW_BASE`], which the head lacks.
fn behind() -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    let mut state = github.state();
    state
        .other_heads
        .insert("trunk".to_string(), NEW_BASE.to_string());
    for (older, newer) in [
        (NEW_BASE, UPDATED),
        (FakeGithub::HEAD, UPDATED),
        (NEW_BASE, RESOLVED),
        (FakeGithub::HEAD, RESOLVED),
    ] {
        state
            .descendants
            .insert((older.to_string(), newer.to_string()));
    }
    drop(state);
    github
}

fn repository() -> Repository {
    Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "trunk".to_string(),
    }
}

/// Run the `update` Step for `input`, recorded as started.
async fn run_update(
    agent: &FakeAgent,
    github: &FakeGithub,
    repo: &FakeRepo,
    input: StepInput<'_>,
) -> Result<StepReport, BuiltInStepError> {
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let entry = EntryId::try_from("update".to_string()).expect("valid");
    repo.start_step(input.key, &entry, 1).await.expect("start");
    steps.run(&step("update"), input).await
}

/// A base GitHub merges in, and nothing else on the branch, ends `done`:
/// the Run's CI runs again and its merge Gate asks again. The writer is
/// told the head to check, which the checks run on.
#[tokio::test(start_paused = true)]
async fn a_base_github_merges_in_ends_done() {
    let github = behind();
    github.state().updated_head = Some(UPDATED.to_string());
    let agent = FakeAgent::ending_with("done");
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);

    let report = run_update(&agent, &github, &FakeRepo::default(), update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "done");
    assert_eq!(github.state().updates, vec![FakeGithub::HEAD.to_string()]);
    let tasks = agent.tasks.lock().expect("tasks");
    assert_eq!(tasks[0].role.as_str(), "writer");
    assert!(tasks[0].prompt.contains(UPDATED), "{}", tasks[0].prompt);
    assert!(tasks[0].prompt.contains("Change nothing"));
}

/// A branch that changed since GitHub's merge, as when a fix of the checks
/// was pushed, ends `resolved`: the change is reviewed again.
#[tokio::test]
async fn a_branch_changed_after_the_merge_ends_resolved() {
    let github = behind();
    github.state().head = Some(RESOLVED.to_string());
    let repo = FakeRepo::default();
    let agent = FakeAgent::ending_with("done");
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);
    repo.keep_branch_update(
        update.key,
        &BranchUpdate {
            base_sha: NEW_BASE.to_string(),
            from_head: FakeGithub::HEAD.to_string(),
            conflict: false,
            updated_head: Some(UPDATED.to_string()),
        },
    )
    .await
    .expect("keep");

    let report = run_update(&agent, &github, &repo, update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "resolved");
    assert!(
        github.state().updates.is_empty(),
        "a kept update is not asked again"
    );
}

/// A conflict goes to the writer, told to merge the base in, never to
/// rebase or force-push; what it pushes is reviewed again once it holds
/// both the base and every commit the branch had.
#[tokio::test]
async fn a_conflict_the_writer_resolves_ends_resolved() {
    let github = behind();
    github.state().update_answer = Some(BranchUpdating::Refused(
        "merge conflict between base and head".to_string(),
    ));
    github.state().head = Some(RESOLVED.to_string());
    let agent = FakeAgent::ending_with("done");
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);

    let report = run_update(&agent, &github, &FakeRepo::default(), update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "resolved");
    let tasks = agent.tasks.lock().expect("tasks");
    assert!(tasks[0].prompt.contains(&format!("git merge {NEW_BASE}")));
    assert!(tasks[0].prompt.contains("Never rebase"));
    let offered: Vec<&str> = tasks[0].outcomes.iter().map(|o| o.as_str()).collect();
    assert_eq!(offered, ["done", "conflict"]);
}

/// A writer that cannot resolve the conflict, or says it did but pushed a
/// branch that lacks the base or a commit it had, leaves it to a person.
#[tokio::test]
async fn a_conflict_no_one_resolved_goes_to_a_person() {
    let repository = repository();

    let gave_up = behind();
    gave_up.state().update_answer = Some(BranchUpdating::Refused("conflict".to_string()));
    let mut update = input(&[]);
    update.repository = Some(&repository);
    let report = run_update(
        &FakeAgent::ending_with("conflict"),
        &gave_up,
        &FakeRepo::default(),
        update,
    )
    .await
    .expect("update");
    assert_eq!(report.outcome.as_str(), "conflict");

    let lacking = behind();
    lacking.state().update_answer = Some(BranchUpdating::Refused("conflict".to_string()));
    // A head that holds neither: a rewrite, or no merge at all.
    lacking.state().head = Some("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string());
    let mut update = input(&[]);
    update.repository = Some(&repository);
    let report = run_update(
        &FakeAgent::ending_with("done"),
        &lacking,
        &FakeRepo::default(),
        update,
    )
    .await
    .expect("update");
    assert_eq!(report.outcome.as_str(), "conflict");
    assert!(
        report.output.contains("a person resolves it"),
        "{}",
        report.output
    );
}
