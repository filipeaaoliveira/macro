//! The `update` Step: the base merged in by GitHub, or a conflict the
//! writer resolves or hands to a person.

use factory_records::domain::merge_queue::{BranchUpdate, MergeQueueRepository};
use factory_records::domain::models::Repository;

use factory_records::domain::models::{EntryId, RunId};
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
    // GitHub's "Update branch" merge: the head it read, then the base.
    state.parents.insert(
        UPDATED.to_string(),
        vec![FakeGithub::HEAD.to_string(), NEW_BASE.to_string()],
    );
    drop(state);
    github
}

/// Records whose Run's review last passed on [`FakeGithub::HEAD`].
async fn reviewed_at_head(run_id: RunId) -> FakeRepo {
    let repo = FakeRepo::default();
    repo.set_reviewed_head(run_id, FakeGithub::HEAD)
        .await
        .expect("reviewed");
    repo
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
    let repo = reviewed_at_head(update.key.run_id).await;

    let report = run_update(&agent, &github, &repo, update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "done", "{}", report.output);
    assert_eq!(github.state().updates, vec![FakeGithub::HEAD.to_string()]);
    assert_eq!(
        repo.reviewed_head(update.key.run_id)
            .await
            .expect("read")
            .as_deref(),
        Some(UPDATED),
        "GitHub's merge into the reviewed head stands for it"
    );
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
    github.state().update_answer = Some(BranchUpdating::Conflict(
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
    gave_up.state().update_answer = Some(BranchUpdating::Conflict("conflict".to_string()));
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
    lacking.state().update_answer = Some(BranchUpdating::Conflict("conflict".to_string()));
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

/// A branch that holds its base already when the update starts, as once a
/// person resolved the conflict and answered the Gate's `retry`, holds
/// commits the factory did not make: it ends `resolved`, so it is reviewed
/// and accepted again, never sent through CI alone to a merge.
#[tokio::test]
async fn a_branch_someone_else_brought_up_to_date_ends_resolved() {
    let github = behind();
    {
        let mut state = github.state();
        state.head = Some(RESOLVED.to_string());
        if let Some(pull_request) = state.pull_request.as_mut() {
            pull_request.head_sha = RESOLVED.to_string();
        }
    }
    let agent = FakeAgent::ending_with("done");
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);

    let report = run_update(&agent, &github, &FakeRepo::default(), update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "resolved");
    assert!(
        report.output.contains("the factory did not make"),
        "{}",
        report.output
    );
    assert!(github.state().updates.is_empty(), "nothing to merge");
}

/// A person's resolution of a conflict, pushed on a base that moved again
/// before their `retry`, has GitHub merge the new base into the person's
/// head: the merge's first parent is no head a review passed on, so the
/// Step ends `resolved` and the resolution is reviewed.
#[tokio::test(start_paused = true)]
async fn a_moved_base_merged_into_a_person_s_resolution_ends_resolved() {
    let github = behind();
    let resolution = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    {
        let mut state = github.state();
        state.head = Some(resolution.to_string());
        if let Some(pull_request) = state.pull_request.as_mut() {
            pull_request.head_sha = resolution.to_string();
        }
        state.updated_head = Some(UPDATED.to_string());
        state.parents.insert(
            UPDATED.to_string(),
            vec![resolution.to_string(), NEW_BASE.to_string()],
        );
        state
            .descendants
            .insert((resolution.to_string(), UPDATED.to_string()));
    }
    let agent = FakeAgent::ending_with("done");
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);
    let repo = reviewed_at_head(update.key.run_id).await;

    let report = run_update(&agent, &github, &repo, update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "resolved", "{}", report.output);
    assert_eq!(github.state().updates, vec![resolution.to_string()]);
    assert!(
        report.output.contains("no review has seen"),
        "{}",
        report.output
    );
    assert_eq!(
        repo.reviewed_head(update.key.run_id)
            .await
            .expect("read")
            .as_deref(),
        Some(FakeGithub::HEAD),
        "the reviewed head stays the one the review passed on"
    );
}

/// A push while GitHub's merge was awaited, which itself holds the base, is
/// not taken for GitHub's merge: its parents are not the reviewed head and
/// the base, so the Step ends `resolved`.
#[tokio::test(start_paused = true)]
async fn a_push_that_holds_the_base_during_the_wait_ends_resolved() {
    let github = behind();
    let pushed = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    let own = "ffffffffffffffffffffffffffffffffffffffff";
    {
        let mut state = github.state();
        // GitHub takes the request, and someone's push lands first: a merge
        // of the base into their own commit on the branch.
        state.update_answer = Some(BranchUpdating::Started);
        state.head = Some(pushed.to_string());
        state.parents.insert(
            pushed.to_string(),
            vec![own.to_string(), NEW_BASE.to_string()],
        );
        state
            .descendants
            .insert((NEW_BASE.to_string(), pushed.to_string()));
    }
    let agent = FakeAgent::ending_with("done");
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);
    let repo = reviewed_at_head(update.key.run_id).await;

    let report = run_update(&agent, &github, &repo, update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "resolved", "{}", report.output);
}

/// An update asked again from a head someone pushed, as when GitHub's first
/// merge never showed, merges the base into that head: the merge's first
/// parent is the push, not the reviewed head, so it ends `resolved`.
#[tokio::test(start_paused = true)]
async fn an_update_asked_again_from_a_pushed_head_ends_resolved() {
    let github = behind();
    let pushed = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    {
        let mut state = github.state();
        state.head = Some(pushed.to_string());
        if let Some(pull_request) = state.pull_request.as_mut() {
            pull_request.head_sha = pushed.to_string();
        }
        state.updated_head = Some(UPDATED.to_string());
        state.parents.insert(
            UPDATED.to_string(),
            vec![pushed.to_string(), NEW_BASE.to_string()],
        );
        state
            .descendants
            .insert((pushed.to_string(), UPDATED.to_string()));
    }
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);
    let repo = reviewed_at_head(update.key.run_id).await;
    // The first ask, from the reviewed head, never showed.
    repo.keep_branch_update(
        update.key,
        &BranchUpdate {
            base_sha: NEW_BASE.to_string(),
            from_head: FakeGithub::HEAD.to_string(),
            conflict: false,
            updated_head: None,
        },
    )
    .await
    .expect("keep");

    let report = run_update(&FakeAgent::ending_with("done"), &github, &repo, update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "resolved", "{}", report.output);
    assert_eq!(github.state().updates, vec![pushed.to_string()]);
    assert_eq!(
        repo.reviewed_head(update.key.run_id)
            .await
            .expect("read")
            .as_deref(),
        Some(FakeGithub::HEAD)
    );
}

/// A Run that keeps no reviewed head, as one whose review passed before the
/// factory kept it, is reviewed again after any update.
#[tokio::test(start_paused = true)]
async fn an_update_of_a_run_with_no_reviewed_head_ends_resolved() {
    let github = behind();
    github.state().updated_head = Some(UPDATED.to_string());
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);

    let report = run_update(
        &FakeAgent::ending_with("done"),
        &github,
        &FakeRepo::default(),
        update,
    )
    .await
    .expect("update");

    assert_eq!(report.outcome.as_str(), "resolved", "{}", report.output);
}

/// GitHub refusing the update for a reason other than a conflict, as for a
/// token that lacks a scope, ends `conflict` with GitHub's message, which
/// opens the entry's Gate for a person: the Run does not fail, and no agent
/// is asked.
#[tokio::test]
async fn an_update_github_refuses_goes_to_a_person() {
    let github = behind();
    github.state().update_answer = Some(BranchUpdating::Declined(
        "Resource not accessible by integration".to_string(),
    ));
    let agent = FakeAgent::ending_with("done");
    let repository = repository();
    let mut update = input(&[]);
    update.repository = Some(&repository);

    let report = run_update(&agent, &github, &FakeRepo::default(), update)
        .await
        .expect("update");

    assert_eq!(report.outcome.as_str(), "conflict");
    assert!(
        report
            .output
            .contains("Resource not accessible by integration"),
        "{}",
        report.output
    );
    assert!(agent.tasks.lock().expect("tasks").is_empty());
}
