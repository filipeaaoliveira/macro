//! The Run's branch once its pull request merged: deleted, as GitHub's
//! "delete branch after merge" does, whoever merged it; kept when the Run
//! ends without merging it.

use factory_records::domain::models::RunId;
use factory_records::domain::ports::RunRepository;

use super::{input, outcome, steps};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::models::PullRequestState;

/// GitHub with the Run's pull request open and passing, from its branch at
/// the pull request's head, changing no Control File.
fn github() -> FakeGithub {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    github.state().changed = Some(Vec::new());
    github.state().head = Some(FakeGithub::HEAD.to_string());
    github
}

/// A Run whose person agreed to merge the pull request's head.
async fn agreed(repo: &FakeRepo) -> RunId {
    let run_id = RunId::generate();
    repo.set_merge_head(run_id, FakeGithub::HEAD)
        .await
        .expect("keep");
    run_id
}

/// Mark the pull request merged, as a person on GitHub does.
fn merged_on_github(github: &FakeGithub) {
    if let Some(pull_request) = github.state().pull_request.as_mut() {
        pull_request.state = PullRequestState::Merged;
    }
}

/// The merge deletes the Run's own branch once merged, and only it; a
/// retry finds the branch gone and deletes nothing again.
#[tokio::test]
async fn merging_deletes_the_run_s_branch() {
    let github = github();
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let first = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");
    assert_eq!(outcome(&first), "merged");
    assert_eq!(
        github.state().deleted_branches,
        [format!("factory/{run_id}")]
    );

    // Pull requests based on the Run's branch were looked for, not on its
    // base.
    assert_eq!(github.state().onto_asked, [format!("factory/{run_id}")]);

    let again = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge again");
    assert_eq!(outcome(&again), "merged");
    assert_eq!(
        github.state().deleted_branches,
        [format!("factory/{run_id}")]
    );
}

/// A merge the Run made deletes its branch whatever the repository's own
/// setting: GitHub's deletion, if it came first, leaves nothing to delete.
#[tokio::test]
async fn merging_deletes_the_branch_without_reading_the_repository_s_setting() {
    let github = github();
    github.state().deletes_merged_branches = true;
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");

    assert_eq!(github.state().access_reads, 0);
    assert_eq!(
        github.state().deleted_branches,
        [format!("factory/{run_id}")]
    );
}

/// A pull request a person merged on GitHub, found by `ready_to_merge` or
/// `merge`, has the Run's branch deleted too.
#[tokio::test]
async fn a_pull_request_merged_outside_the_run_has_its_branch_deleted() {
    for step in ["ready_to_merge", "merge"] {
        let github = github();
        merged_on_github(&github);
        let repo = FakeRepo::default();
        let run_id = agreed(&repo).await;

        let steps = steps(&github, &repo);
        let report = match step {
            "ready_to_merge" => steps.ready_to_merge(input(run_id)).await,
            _ => steps.merge(input(run_id)).await,
        }
        .expect("step");

        assert_eq!(outcome(&report), "merged", "{step}");
        assert!(github.state().merges.is_empty(), "{step} merges nothing");
        assert_eq!(
            github.state().deleted_branches,
            [format!("factory/{run_id}")],
            "{step} deletes the branch"
        );
    }
}

/// A repository that deletes merged branches itself has the branch of a
/// pull request merged outside the Run left to GitHub.
#[tokio::test]
async fn a_repository_that_deletes_merged_branches_itself_is_left_to_it() {
    let github = github();
    github.state().deletes_merged_branches = true;
    merged_on_github(&github);
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let report = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "merged");
    assert!(github.state().deleted_branches.is_empty());
}

/// A branch pushed to after its pull request merged holds commits no one
/// merged: it is kept.
#[tokio::test]
async fn a_branch_that_moved_since_its_merge_is_kept() {
    let github = github();
    merged_on_github(&github);
    github.state().head = Some("4444444444444444444444444444444444444444".to_string());
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let report = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "merged");
    assert!(github.state().deleted_branches.is_empty());
}

/// A branch an open pull request is based on is kept: deleting it through
/// the API would close that pull request.
#[tokio::test]
async fn a_branch_another_pull_request_is_based_on_is_kept() {
    let github = github();
    github.state().based_on_branch = true;
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let report = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "merged");
    assert!(github.state().deleted_branches.is_empty());
}

/// A branch whose pull requests could not be looked for is kept, and the
/// merge stands.
#[tokio::test]
async fn a_branch_whose_pull_requests_cannot_be_read_is_kept() {
    let github = github();
    github.state().onto_fails = true;
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let report = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "merged");
    assert_eq!(github.state().merges, [FakeGithub::HEAD]);
    assert!(github.state().deleted_branches.is_empty());
}

/// A pull request merged outside the Run, in a repository the owner can no
/// longer read, keeps its branch.
#[tokio::test]
async fn a_repository_out_of_reach_keeps_the_branch() {
    let github = github();
    merged_on_github(&github);
    github.state().unreachable = true;
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let report = steps(&github, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&report), "merged");
    assert!(github.state().deleted_branches.is_empty());
}

/// A head that cannot be read keeps the branch, and the merge stands; so
/// does, for a pull request merged outside the Run, a repository whose
/// settings cannot be read.
#[tokio::test]
async fn a_read_that_fails_keeps_the_branch() {
    let head_unread = github();
    head_unread.state().branch_head_fails = true;
    let access_unread = github();
    access_unread.state().access_fails = true;
    merged_on_github(&access_unread);
    let repo = FakeRepo::default();

    let run_id = agreed(&repo).await;
    let merged = steps(&head_unread, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");
    let run_id = agreed(&repo).await;
    let found = steps(&access_unread, &repo)
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");

    assert_eq!(outcome(&merged), "merged");
    assert_eq!(head_unread.state().merges, [FakeGithub::HEAD]);
    assert_eq!(outcome(&found), "merged");
    assert!(head_unread.state().deleted_branches.is_empty());
    assert!(access_unread.state().deleted_branches.is_empty());
}

/// A deletion GitHub does not answer is given up after a minute, and the
/// merge stands.
#[tokio::test(start_paused = true)]
async fn a_deletion_github_does_not_answer_is_given_up() {
    let github = github();
    github.state().delete_gate = Some(std::sync::Arc::new(tokio::sync::Semaphore::new(0)));
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let steps = steps(&github, &repo);
    let merging = steps.merge(input(run_id));
    let report = tokio::time::timeout(std::time::Duration::from_secs(120), merging)
        .await
        .expect("the merge ends")
        .expect("merge");

    assert_eq!(outcome(&report), "merged");
    assert!(github.state().deleted_branches.is_empty());
}

/// A branch GitHub does not delete leaves the merge as it is: merged and
/// recorded, the Step done.
#[tokio::test]
async fn a_branch_that_cannot_be_deleted_fails_nothing() {
    let github = github();
    github.state().delete_fails = true;
    let repo = FakeRepo::default();
    let run_id = agreed(&repo).await;

    let report = steps(&github, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");

    assert_eq!(outcome(&report), "merged");
    assert_eq!(github.state().merges, [FakeGithub::HEAD]);
    assert!(github.state().deleted_branches.is_empty());
}

/// A Run that does not merge its pull request keeps its branch, for a
/// person to look at: a merge GitHub refuses, a head that moved, a pull
/// request closed, and the Steps before the merge.
#[tokio::test]
async fn a_run_that_does_not_merge_keeps_its_branch() {
    let refused = github();
    refused.state().merge_refusal = Some("Required reviews are missing.".to_string());
    let moved = github();
    let closed = github();
    if let Some(pull_request) = closed.state().pull_request.as_mut() {
        pull_request.state = PullRequestState::Closed;
    }
    let repo = FakeRepo::default();

    let run_id = agreed(&repo).await;
    let report = steps(&refused, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");
    assert_eq!(outcome(&report), "refused");

    let run_id = RunId::generate();
    repo.set_merge_head(run_id, "2222222222222222222222222222222222222222")
        .await
        .expect("keep");
    let report = steps(&moved, &repo)
        .merge(input(run_id))
        .await
        .expect("merge");
    assert_eq!(outcome(&report), "changed");

    let run_id = agreed(&repo).await;
    let steps_of_closed = steps(&closed, &repo);
    let published = steps_of_closed
        .publish(input(run_id))
        .await
        .expect("publish");
    let ready = steps_of_closed
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");
    let merge = steps_of_closed.merge(input(run_id)).await.expect("merge");
    assert_eq!(outcome(&published), "closed");
    assert_eq!(outcome(&ready), "closed");
    assert_eq!(outcome(&merge), "closed");

    let run_id = agreed(&repo).await;
    let open = github();
    let steps_of_open = steps(&open, &repo);
    steps_of_open.publish(input(run_id)).await.expect("publish");
    let ready = steps_of_open
        .ready_to_merge(input(run_id))
        .await
        .expect("ready");
    assert_eq!(outcome(&ready), "done");

    for github in [refused, moved, closed, open] {
        assert!(github.state().deleted_branches.is_empty());
    }
}
