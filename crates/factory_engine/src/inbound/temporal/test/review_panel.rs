//! A `code` Run's review panel on a running Temporal.

use std::sync::{Arc, Mutex};

use factory_records::domain::models::RunOutcome;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    FindingKind, ReviewRoundOutcome, ReviewRoundRepository,
};

use super::{CodeAgent, walk_code};
use crate::domain::fakes::{FakeGithub, ci};

/// A review round's reviewers work at once, each in a Step of its own. A
/// Blocker in the change sends it to the fix, whose round passes; a finding
/// outside the change becomes a follow-up task and holds nothing, and the
/// pull request merges.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_review_panel_s_reviewers_work_at_once_and_a_follow_up_holds_nothing() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let mut agent = CodeAgent::new(&github);
    agent.together = Some(Arc::new(tokio::sync::Barrier::new(3)));
    *agent.reviews.lock().expect("reviews") = vec!["changes", "outside", "pass"];
    let records = Arc::new(Mutex::new(None));

    let ended = walk_code(&agent, &github, |run| {
        let records = records.clone();
        async move {
            *records.lock().expect("records") = Some(run.repo.clone());
            run.answer_next_gate("approve").await;
            run.answer_next_gate("approve").await;
        }
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let repo = records.lock().expect("records").clone().expect("records");
    let run_id = *repo.runs.lock().expect("runs").keys().next().expect("run");
    let steps = repo.list_steps(run_id).await.expect("steps");
    let reviewers: Vec<(u16, Option<String>)> = steps
        .iter()
        .filter(|step| step.entry.as_str() == "review")
        .map(|step| {
            (
                step.seq,
                step.end.as_ref().map(|end| end.outcome.to_string()),
            )
        })
        .collect();
    assert_eq!(reviewers.len(), 6);
    assert_eq!(
        reviewers[..3]
            .iter()
            .map(|(seq, _)| *seq)
            .collect::<Vec<_>>(),
        [3, 4, 5]
    );

    let rounds = repo.review_rounds(run_id).await.expect("rounds");
    let ends: Vec<_> = rounds
        .iter()
        .map(|round| round.end.as_ref().map(|end| end.outcome))
        .collect();
    assert_eq!(
        ends,
        [
            Some(ReviewRoundOutcome::Changes),
            Some(ReviewRoundOutcome::Pass)
        ]
    );
    // Which reviewer found which depends on which answered first.
    let mut kinds: Vec<_> = rounds[0]
        .findings
        .iter()
        .map(|finding| (finding.path.as_str(), finding.kind))
        .collect();
    kinds.sort_by_key(|(path, _)| *path);
    assert_eq!(
        kinds,
        [
            ("README.md", Some(FindingKind::FollowUp)),
            ("src/slug.rs", Some(FindingKind::Blocker))
        ]
    );
    assert_eq!(repo.follow_ups.lock().expect("follow ups").len(), 1);
    assert_eq!(github.state().merges, [FakeGithub::HEAD]);
}
