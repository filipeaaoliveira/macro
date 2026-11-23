//! A write or fix whose agent hands back `done` with nothing of its work on
//! the Run's branch on GitHub fails, saying so, rather than ending `done`.

use super::*;

/// The branch changes nothing against the base: the agent committed
/// elsewhere, as on a checkout another Run had moved to its own branch.
#[tokio::test]
async fn a_write_whose_branch_changes_nothing_against_its_base_fails() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(Some(CHECKS), Some(&[]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::failed());
    assert!(
        report.output.starts_with(&format!(
            "The agent said its work was done and pushed, but the run's branch `factory/{}` is at `{HEAD}`, which changes nothing against its base `main`",
            input.key.run_id
        )),
        "{}",
        report.output
    );
    assert!(report.output.contains("Wrote the toggle"));
    assert!(rounds(&repo, input).await.is_empty());
    assert!(agent.turns().is_empty());
}

/// A branch GitHub does not have, even after a wait, fails the attempt for
/// a retry, as a round's read of it does: nothing is recorded.
#[tokio::test]
async fn a_write_whose_branch_is_not_on_github_is_left_to_a_retry() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    github.state().head = None;
    let repo = FakeRepo::default();
    let input = input();

    let failed = run(&agent, &github, &repo, "write", input).await;

    assert!(
        matches!(
            failed,
            Err(super::super::super::BuiltInStepError::Agent {
                retryable: true,
                ..
            })
        ),
        "{failed:?}"
    );
    assert!(rounds(&repo, input).await.is_empty());
}

/// A branch GitHub shows only after a moment holds the work: its checks
/// run, and pass.
#[tokio::test]
async fn a_branch_that_shows_after_the_lag_holds_the_work() {
    let agent = ScriptedAgent::new("done", vec![Ok(("passed", "web: exit 0"))]);
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    github.state().branch_heads = [None, Some(HEAD.to_string())].into();
    let repo = FakeRepo::default();

    let report = run(&agent, &github, &repo, "write", input())
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done(), "{}", report.output);
}

/// An update's agent merges the base in, and a branch at the base's head
/// is no reason to fail: only a write and a fix push work of their own.
#[tokio::test]
async fn work_that_pushes_nothing_of_its_own_is_not_read_off_the_branch() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(Some(CHECKS), Some(&[]));
    let repo = FakeRepo::default();
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let input = input();
    let entry = EntryId::try_from("write".to_string()).expect("valid");
    repo.start_step(input.key, &entry, 1).await.expect("start");
    let step = StepName::try_from("write".to_string()).expect("valid");

    let report = steps
        .work_and_check(
            &step,
            "Merge the base in",
            vec![Outcome::done()],
            input,
            None,
            false,
        )
        .await
        .expect("work");

    assert_eq!(report.outcome, Outcome::done(), "{}", report.output);
}

/// A write that changed nothing says so, and the branch is not read.
#[tokio::test]
async fn an_unchanged_write_does_not_need_a_branch() {
    let agent = ScriptedAgent::new("unchanged", Vec::new());
    let github = github(Some(CHECKS), None);
    github.state().head = None;
    let repo = FakeRepo::default();

    let report = run(&agent, &github, &repo, "write", input())
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::unchanged());
}
