//! A write or fix whose agent hands back `done` with nothing of its work on
//! the Run's branch on GitHub fails, saying so, rather than ending `done`.

use factory_records::domain::review_rounds::{FindingKind, ReviewFinding, ReviewRound};

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

/// A branch still missing when the Step's time for rounds is up ends the
/// Step `failed`, saying why, instead of a last retry that says nothing.
#[tokio::test]
async fn a_branch_still_missing_at_the_step_s_end_fails_saying_so() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    github.state().head = None;
    let repo = FakeRepo::default();
    let input = input();
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let entry = EntryId::try_from("write".to_string()).expect("valid");
    repo.start_step(input.key, &entry, 1).await.expect("start");
    age(&repo, input.key);
    let step = StepName::try_from("write".to_string()).expect("valid");

    let report = steps.run(&step, input).await.expect("write");

    assert_eq!(report.outcome, Outcome::failed());
    assert!(
        report.output.starts_with(&format!(
            "The agent said its work was done and pushed, but the run's branch `factory/{}` is not on GitHub",
            input.key.run_id
        )),
        "{}",
        report.output
    );
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

/// A review round that reviewed [`HEAD`], with two Blockers.
fn reviewed_round() -> ReviewRound {
    let blocker = |place, path: &str| ReviewFinding {
        seq: 3,
        place,
        role: Some(Role::try_from("tests-reviewer".to_string()).expect("valid")),
        path: path.to_string(),
        lines: None,
        problem: format!("{path} skips the empty case"),
        why: "An empty list panics".to_string(),
        blocks_anywhere: false,
        repeats_dropped: false,
        fix_status: None,
        dispute: None,
        ruling: None,
        ruled_by: None,
        kind: Some(FindingKind::Blocker),
        follow_up_task_id: None,
        follow_up_key: None,
        follow_up_commented: false,
    };
    ReviewRound {
        round: 1,
        visit: 1,
        seq: 3,
        reviewers: 1,
        head_sha: Some(HEAD.to_string()),
        diff_base: Some("main".to_string()),
        fix_diff: false,
        failure: None,
        started_at: chrono::Utc::now(),
        end: None,
        findings: vec![blocker(1, "src/list.rs"), blocker(2, "src/map.rs")],
        answered: Vec::new(),
    }
}

/// A fix that leaves the branch where its round reviewed it fails as
/// unpushed unless it disputes every Blocker in its block: a Blocker it
/// does not answer, as one disputed only in prose, needs a commit as one
/// it says it fixed does.
#[tokio::test]
async fn a_fix_that_leaves_a_blocker_unanswered_on_an_unmoved_head_fails() {
    let dispute = |number| {
        format!(
            "{{\"number\": {number}, \"status\": \"disputed\", \"reason\": \"It is generated\"}}"
        )
    };
    let cases = [
        (
            "I dispute both blockers: the files are generated.".to_string(),
            Outcome::failed(),
        ),
        (
            format!(
                "Disputed.\n```json\n{{\"blockers\": [{}]}}\n```",
                dispute(1)
            ),
            Outcome::failed(),
        ),
        (
            format!(
                "Disputed.\n```json\n{{\"blockers\": [{}, {}]}}\n```",
                dispute(1),
                dispute(2)
            ),
            Outcome::done(),
        ),
    ];
    for (said, expected) in cases {
        let mut agent = ScriptedAgent::new("done", Vec::new());
        agent.work_text = said.clone();
        let github = github(None, Some(&["src/list.rs"]));
        let repo = FakeRepo::default();
        let round = reviewed_round();
        let mut input = input();
        input.review = Some(&round);
        repo.review_rounds
            .lock()
            .expect("rounds")
            .push((input.key.run_id, round.clone()));

        let report = run(&agent, &github, &repo, "fix", input)
            .await
            .expect("fix");

        assert_eq!(report.outcome, expected, "{said}: {}", report.output);
        if expected == Outcome::failed() {
            assert!(
                report.output.contains(&format!(
                    "is still at `{HEAD}`, the commit review round 1 reviewed"
                )),
                "{}",
                report.output
            );
        }
    }
}
