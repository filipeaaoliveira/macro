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

/// The branch never reached GitHub.
#[tokio::test]
async fn a_write_whose_branch_is_not_on_github_fails() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    github.state().head = None;
    let repo = FakeRepo::default();
    let input = input();

    let report = run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::failed());
    assert!(
        report.output.contains(&format!(
            "the run's branch `factory/{}` is not on GitHub",
            input.key.run_id
        )),
        "{}",
        report.output
    );
    assert!(rounds(&repo, input).await.is_empty());
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
