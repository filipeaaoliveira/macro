//! A branch behind its base, the merge queue, and the update that brings a
//! branch up to date.

use std::time::Duration;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::merge_queue::{BranchUpdate, MergeQueueRepository};
use factory_records::domain::models::{Outcome, RunId};
use factory_records::domain::ports::RunRepository;

use super::{input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::models::{BranchUpdating, PullRequestState};
use crate::domain::pull_request_steps::{PullRequestStepError, QUEUE_WAIT, Update};

/// The base's head when the branch was made and checked.
const OLD_BASE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
/// A commit the base gained since.
const NEW_BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
/// The head GitHub's merge of the base into the branch makes.
const UPDATED: &str = "cccccccccccccccccccccccccccccccccccccccc";

/// The outcomes the `code` Flow routes from its merge Gate's and merge's
/// entries, so the Steps take the queue's paths.
fn routes() -> Vec<Outcome> {
    [
        "behind", "queued", "merged", "changed", "held", "closed", "refused",
    ]
    .iter()
    .map(|name| Outcome::try_from((*name).to_string()).expect("outcome"))
    .collect()
}

/// GitHub with the Run's pull request open at [`FakeGithub::HEAD`], its
/// checks passed, changing no Control File, on a base at `base`, which the
/// head holds only when `holds`.
fn github_on(base: &str, holds: bool) -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    let mut state = github.state();
    state.changed = Some(Vec::new());
    state
        .other_heads
        .insert("main".to_string(), base.to_string());
    state
        .descendants
        .insert((OLD_BASE.to_string(), FakeGithub::HEAD.to_string()));
    if holds {
        state
            .descendants
            .insert((base.to_string(), FakeGithub::HEAD.to_string()));
    }
    state
        .descendants
        .insert((NEW_BASE.to_string(), UPDATED.to_string()));
    state
        .descendants
        .insert((FakeGithub::HEAD.to_string(), UPDATED.to_string()));
    drop(state);
    github
}

async fn approved(repo: &FakeRepo, run_id: RunId) {
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
}

/// A person is never asked to merge a head behind its base: the merge Gate's
/// Step ends `behind`. A Run on an older Flow, which routes no `behind`, is
/// asked as before.
#[tokio::test]
async fn ready_to_merge_sends_a_branch_behind_its_base_to_be_updated() {
    let github = github_on(NEW_BASE, false);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    approved(&repo, run_id).await;
    let routes = routes();

    let behind = steps(&github, &repo)
        .ready_to_merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("ready");
    let older_flow = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&behind), "behind");
    assert!(behind.output.contains(NEW_BASE), "{}", behind.output);
    assert_eq!(outcome(&older_flow), "done");

    let current = github_on(OLD_BASE, true);
    let asked = steps(&current, &repo)
        .ready_to_merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("ready");
    assert_eq!(outcome(&asked), "done");
}

/// The head a person agreed to merge is not merged once the base gained a
/// commit since its checks passed: the Step ends `behind`, merges nothing,
/// and the Run keeps its place in the queue.
#[tokio::test]
async fn an_approved_head_the_base_moved_past_is_never_merged() {
    let github = github_on(NEW_BASE, false);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    approved(&repo, run_id).await;
    let routes = routes();

    let report = steps(&github, &repo)
        .merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "behind");
    assert!(github.state().merges.is_empty());
    let queued = repo.queue();
    assert_eq!(queued.queued.len(), 1);
    assert!(
        queued.queued[0].left_at.is_some(),
        "it left, keeping its place"
    );
}

/// A head that holds its base merges, and leaves the queue.
#[tokio::test]
async fn an_up_to_date_head_merges_and_leaves_the_queue() {
    let github = github_on(OLD_BASE, true);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    approved(&repo, run_id).await;
    let routes = routes();

    let report = steps(&github, &repo)
        .merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "merged");
    assert_eq!(github.state().merges, vec![FakeGithub::HEAD.to_string()]);
    assert!(repo.queue().queued.is_empty());
}

/// GitHub refusing a merge because the base moved under it, as a branch
/// protection that wants branches up to date does, is a branch behind its
/// base; a refusal for another reason stays one.
#[tokio::test]
async fn a_merge_refused_for_a_base_that_moved_is_behind() {
    let routes = routes();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    approved(&repo, run_id).await;

    let moved = github_on(OLD_BASE, true);
    moved.state().merge_refusal = Some("Base branch was modified.".to_string());
    moved.state().main_moves_on_merge = Some(NEW_BASE.to_string());
    let behind = steps(&moved, &repo)
        .merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("merge");
    assert_eq!(outcome(&behind), "behind");

    let refused = github_on(OLD_BASE, true);
    refused.state().merge_refusal = Some("Required reviews are missing.".to_string());
    let report = steps(&refused, &repo)
        .merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("merge");
    assert_eq!(outcome(&report), "refused");
}

/// Two Runs at their merge into one base merge one after the other: the
/// second waits while the first is ahead of it, and merges once the first
/// merged.
#[tokio::test(start_paused = true)]
async fn the_second_run_in_the_queue_waits_for_the_first() {
    let github = github_on(OLD_BASE, true);
    let repo = FakeRepo::default();
    let first = RunId::generate();
    let second = RunId::generate();
    approved(&repo, second).await;
    // The first reached its merge before the second, and is merging.
    repo.enter_merge_queue(first).await.expect("enter");

    let waiting = tokio::spawn({
        let step = steps(&github, &repo);
        async move {
            let routes = routes();
            step.merge(crate::domain::models::StepInput {
                routes: &routes,
                ..input(second)
            })
            .await
        }
    });
    tokio::time::sleep(Duration::from_secs(90)).await;
    assert!(
        github.state().merges.is_empty(),
        "the second waits while the first is ahead"
    );

    repo.drop_from_merge_queue(first)
        .await
        .expect("first merged");
    let report = waiting.await.expect("join").expect("merge");

    assert_eq!(outcome(&report), "merged");
    assert_eq!(github.state().merges, vec![FakeGithub::HEAD.to_string()]);
}

/// A Run whose turn does not come within the queue's wait merges nothing
/// and ends `queued`, keeping its place.
#[tokio::test(start_paused = true)]
async fn a_run_whose_turn_does_not_come_ends_queued() {
    let github = github_on(OLD_BASE, true);
    let repo = FakeRepo::default();
    let first = RunId::generate();
    let second = RunId::generate();
    approved(&repo, second).await;
    repo.enter_merge_queue(first).await.expect("enter");
    let routes = routes();

    let started = tokio::time::Instant::now();
    let report = steps(&github, &repo)
        .merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(second)
        })
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "queued");
    // The Run ahead may be another person's: its id, and how many Runs are
    // ahead, stay out of what this Run's readers see.
    assert!(
        !report.output.contains(&first.to_string()),
        "{}",
        report.output
    );
    let (_, note) = report.output.split_once("\n\n").expect("a note");
    assert!(!note.chars().any(|c| c.is_ascii_digit()), "{note}");
    assert!(
        report.output.contains("have not merged yet"),
        "{}",
        report.output
    );
    assert!(started.elapsed() <= QUEUE_WAIT);
    assert!(github.state().merges.is_empty());
    assert_eq!(
        repo.merge_queue_ahead(second, chrono::TimeDelta::minutes(90))
            .await
            .expect("ahead"),
        vec![first],
        "both keep their places"
    );
}

/// GitHub merges the base into a branch behind it, from the head it read,
/// and the update keeps the head its merge made; a retry finds it kept and
/// asks GitHub nothing more.
#[tokio::test(start_paused = true)]
async fn an_update_merges_the_base_in_and_a_retry_goes_on_from_it() {
    let github = github_on(NEW_BASE, false);
    github.state().updated_head = Some(UPDATED.to_string());
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let step = steps(&github, &repo);

    let update = step.update_branch(&input(run_id)).await.expect("update");
    let again = step.update_branch(&input(run_id)).await.expect("again");

    let merged = Update::Merged(BranchUpdate {
        base_sha: NEW_BASE.to_string(),
        from_head: FakeGithub::HEAD.to_string(),
        conflict: false,
        updated_head: Some(UPDATED.to_string()),
    });
    assert_eq!(update, merged);
    assert_eq!(again, merged);
    assert_eq!(github.state().updates, vec![FakeGithub::HEAD.to_string()]);
}

/// A base GitHub cannot merge into the branch, as for a conflict, is kept
/// as a conflict for the agent; a retry reads it kept.
#[tokio::test]
async fn an_update_github_cannot_merge_is_a_conflict() {
    let github = github_on(NEW_BASE, false);
    github.state().update_answer = Some(BranchUpdating::Conflict(
        "merge conflict between base and head".to_string(),
    ));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let step = steps(&github, &repo);

    let update = step.update_branch(&input(run_id)).await.expect("update");
    github.state().update_answer = None;
    let again = step.update_branch(&input(run_id)).await.expect("again");

    let conflict = Update::Conflict(BranchUpdate {
        base_sha: NEW_BASE.to_string(),
        from_head: FakeGithub::HEAD.to_string(),
        conflict: true,
        updated_head: None,
    });
    assert_eq!(update, conflict);
    assert_eq!(again, conflict);
    assert_eq!(github.state().updates.len(), 1);
}

/// A head someone pushed while the update kept waited for GitHub's merge.
const PUSHED: &str = "dddddddddddddddddddddddddddddddddddddddd";

/// An update GitHub took but whose merge never shows, because someone
/// pushed in between, is not polled for until retries run out: once its
/// wait is over the branch is read again and GitHub asked again from the
/// head it is at now, and the update kept is replaced by that one.
#[tokio::test(start_paused = true)]
async fn an_update_that_never_shows_is_asked_again_from_the_branch_now() {
    let github = github_on(NEW_BASE, false);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let key = input(run_id).key;
    repo.keep_branch_update(
        key,
        &BranchUpdate {
            base_sha: NEW_BASE.to_string(),
            from_head: FakeGithub::HEAD.to_string(),
            conflict: false,
            updated_head: None,
        },
    )
    .await
    .expect("keep");
    {
        let mut state = github.state();
        state.head = Some(PUSHED.to_string());
        if let Some(pull_request) = state.pull_request.as_mut() {
            pull_request.head_sha = PUSHED.to_string();
        }
        state.updated_head = Some(UPDATED.to_string());
        state
            .descendants
            .insert((PUSHED.to_string(), UPDATED.to_string()));
    }

    let update = steps(&github, &repo)
        .update_branch(&input(run_id))
        .await
        .expect("update");

    let merged = BranchUpdate {
        base_sha: NEW_BASE.to_string(),
        from_head: PUSHED.to_string(),
        conflict: false,
        updated_head: Some(UPDATED.to_string()),
    };
    assert_eq!(update, Update::Merged(merged.clone()));
    assert_eq!(github.state().updates, vec![PUSHED.to_string()]);
    assert_eq!(repo.queue().updates.get(&key), Some(&merged));
}

/// GitHub taking an update it never applies is asked again within the
/// attempt; still nothing shown fails the attempt for a retry, which asks
/// the same way.
#[tokio::test(start_paused = true)]
async fn an_update_github_never_applies_is_asked_again() {
    let github = github_on(NEW_BASE, false);
    // GitHub answers it took the update, and never merges.
    github.state().update_answer = Some(BranchUpdating::Started);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let first = steps(&github, &repo).update_branch(&input(run_id)).await;

    assert!(
        matches!(first, Err(PullRequestStepError::BranchMoving(_))),
        "{first:?}"
    );
    assert_eq!(
        github.state().updates,
        vec![FakeGithub::HEAD.to_string(), FakeGithub::HEAD.to_string()],
        "asked once, then again once its merge did not show"
    );
}

/// GitHub declining an update for a reason other than a conflict, as for
/// a token that lacks a scope, is no conflict for the writer: it is its
/// refusal with GitHub's reason, for a person, and nothing is kept.
#[tokio::test]
async fn an_update_github_declines_is_no_conflict() {
    let github = github_on(NEW_BASE, false);
    github.state().update_answer = Some(BranchUpdating::Declined(
        "Resource not accessible by integration".to_string(),
    ));
    let repo = FakeRepo::default();

    let declined = steps(&github, &repo)
        .update_branch(&input(RunId::generate()))
        .await
        .expect("update");

    assert_eq!(
        declined,
        Update::Declined("Resource not accessible by integration".to_string())
    );
    assert!(repo.queue().updates.is_empty());
}

/// A push to the branch since it was read is never merged over: the update
/// is left to a retry, which reads the branch again, and keeps nothing.
#[tokio::test]
async fn an_update_never_merges_over_a_push() {
    let github = github_on(NEW_BASE, false);
    github.state().update_answer = Some(BranchUpdating::HeadMoved);
    let repo = FakeRepo::default();
    let run_id = RunId::generate();

    let moved = steps(&github, &repo).update_branch(&input(run_id)).await;

    assert!(
        matches!(moved, Err(PullRequestStepError::BranchMoving(_))),
        "{moved:?}"
    );
    assert!(repo.queue().updates.is_empty());
}

/// A branch already up to date has nothing merged into it; its checks still
/// run again. A pull request merged meanwhile ends the Step `merged`.
#[tokio::test]
async fn an_update_of_a_branch_up_to_date_or_merged_merges_nothing() {
    let current = github_on(NEW_BASE, true);
    let repo = FakeRepo::default();
    let update = steps(&current, &repo)
        .update_branch(&input(RunId::generate()))
        .await
        .expect("update");
    assert!(
        matches!(&update, Update::Merged(kept) if kept.updated_head.as_deref() == Some(FakeGithub::HEAD)),
        "{update:?}"
    );
    assert!(current.state().updates.is_empty());

    let merged = github_on(NEW_BASE, false);
    if let Some(pull_request) = merged.state().pull_request.as_mut() {
        pull_request.state = PullRequestState::Merged;
    }
    let ended = steps(&merged, &repo)
        .update_branch(&input(RunId::generate()))
        .await
        .expect("update");
    assert!(
        matches!(&ended, Update::Ended(report) if report.outcome.as_str() == "merged"),
        "{ended:?}"
    );
}

/// A hold set on the base while a Run waited for its turn holds it still:
/// it merges nothing and goes back to its Gate at once, within the one wait
/// the hold and the queue share.
#[tokio::test(start_paused = true)]
async fn a_hold_set_while_a_run_waited_its_turn_holds_it() {
    let github = github_on(OLD_BASE, true);
    let repo = FakeRepo::default();
    let first = RunId::generate();
    let second = RunId::generate();
    approved(&repo, second).await;
    repo.enter_merge_queue(first).await.expect("enter");

    let started = tokio::time::Instant::now();
    let waiting = tokio::spawn({
        let step = steps(&github, &repo);
        async move {
            let routes = routes();
            step.merge(crate::domain::models::StepInput {
                routes: &routes,
                ..input(second)
            })
            .await
        }
    });
    tokio::time::sleep(Duration::from_secs(90)).await;
    repo.hold_main(
        &super::REPOSITORY,
        NEW_BASE,
        "- test (failure)",
        chrono::Utc::now(),
    )
    .await
    .expect("hold");
    repo.drop_from_merge_queue(first)
        .await
        .expect("first merged");
    let report = waiting.await.expect("join").expect("merge");

    assert_eq!(outcome(&report), "held");
    assert!(started.elapsed() < QUEUE_WAIT);
    assert!(github.state().merges.is_empty());
}

/// A merge that goes on with its approval after waiting in the queue merges
/// the head approved only while its checks still pass: a check that failed
/// since sends it back through its checks, merging nothing.
#[tokio::test]
async fn a_queued_merge_whose_checks_turned_red_is_not_merged() {
    let github = github_on(OLD_BASE, true);
    github.state().checks = [ci(Some("failure"))].into();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    approved(&repo, run_id).await;
    let routes = routes();

    let report = steps(&github, &repo)
        .merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "changed");
    assert!(github.state().merges.is_empty());
}

/// A merge that goes on with its approval after waiting in the queue merges
/// nothing once the pull request's head moved from the head approved, even
/// with every check on the new head passing: its checks run again and the
/// merge Gate asks again.
#[tokio::test]
async fn a_queued_merge_whose_head_moved_is_not_merged() {
    let github = github_on(OLD_BASE, true);
    if let Some(pull_request) = github.state().pull_request.as_mut() {
        pull_request.head_sha = UPDATED.to_string();
    }
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    approved(&repo, run_id).await;
    let routes = routes();

    let report = steps(&github, &repo)
        .merge(crate::domain::models::StepInput {
            routes: &routes,
            ..input(run_id)
        })
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "changed");
    assert!(report.output.contains(UPDATED), "{}", report.output);
    assert!(github.state().merges.is_empty(), "GitHub is never asked");
}
