use factory_records::domain::standing_rules::RunRules;
use std::num::NonZeroU8;

use factory_records::domain::models::{
    EntryId, NewRun, Outcome, Repository, Role, RunId, RunInsert, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    Finding, FindingKind, Lines, ReviewRoundOutcome, ReviewRoundRepository,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{ChangedFile, ReviewRoundClosing, ReviewRoundOpening, ReviewRoundsImpl, in_diff};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::flow::{StepName, StepUse};
use crate::domain::models::{StepError, StepInput, StepReport, StepRequest};
use crate::domain::ports::{ReviewService, StepExecutor, StepService};
use crate::domain::service::StepServiceImpl;

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid outcome")
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user")
}

fn role(name: &str) -> Role {
    Role::try_from(name.to_string()).expect("valid role")
}

const TASK: Uuid = Uuid::from_u128(0x0000_0000_0000_7000_8000_0000_0000_0101);

/// A `code` Run of Julia's on a task, on `macro/app` from `main`.
async fn code_run(repo: &FakeRepo) -> RunId {
    repo.tasks.lock().expect("tasks").insert(TASK);
    let id = RunId::generate();
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id,
            flow: "code",
            request: "Add dark mode",
            user_id: &julia(),
            task_id: Some(TASK),
            project_id: None,
            repository: Some(&repository),
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    else {
        panic!("the task exists");
    };
    id
}

/// The Steps of the round that starts at `seq`, one per reviewer of
/// `roles`, started.
async fn start_reviewers(repo: &FakeRepo, run_id: RunId, seq: u16, roles: &[&str]) {
    let entry = EntryId::try_from("review".to_string()).expect("entry");
    for (offset, name) in (0..).zip(roles) {
        let key = StepKey {
            run_id,
            seq: seq + offset,
        };
        repo.start_step(key, &entry, 1).await.expect("start");
        repo.with_step_role(key, role(name));
    }
}

fn finding(path: &str, lines: Option<(u32, u32)>) -> Finding {
    Finding {
        path: path.to_string(),
        lines: lines.map(|(start, end)| Lines { start, end }),
        problem: format!("{path} drops the error"),
        why: "A failed save looks like a saved one".to_string(),
        blocks_anywhere: false,
    }
}

fn opening(run_id: RunId, seq: u16, round: u16) -> ReviewRoundOpening {
    ReviewRoundOpening {
        run_id,
        seq,
        round,
        reviewers: 2,
    }
}

fn closing(run_id: RunId, seq: u16) -> ReviewRoundClosing {
    ReviewRoundClosing {
        run_id,
        seq,
        failed: false,
        last: false,
    }
}

fn rounds(
    repo: &FakeRepo,
    github: &FakeGithub,
) -> ReviewRoundsImpl<FakeRepo, FakeGithub, FakeRepo> {
    ReviewRoundsImpl::new(repo.clone(), github.clone(), repo.clone())
}

#[test]
fn a_finding_is_in_the_diff_when_its_lines_meet_a_hunk_of_its_file() {
    let files = [
        ChangedFile::from_patch(
            "src/save.rs".to_string(),
            None,
            Some("@@ -10,4 +12,6 @@ fn save()\n a\n+b\n@@ -40 +44,0 @@\n-gone"),
        ),
        ChangedFile::from_patch("logo.png".to_string(), Some("old.png".to_string()), None),
    ];
    let lines = |start, end| Some(Lines { start, end });

    assert!(in_diff("src/save.rs", lines(17, 30), &files));
    assert!(in_diff("./src/save.rs", lines(44, 44), &files));
    assert!(in_diff("src/save.rs", None, &files));
    assert!(!in_diff("src/save.rs", lines(18, 43), &files));
    assert!(!in_diff("src/load.rs", lines(12, 12), &files));
    // A file with no hunks counts as changed throughout, under either name.
    assert!(in_diff("old.png", lines(1, 1), &files));
}

/// The first round reviews the Run's whole change against its base; the
/// next reviews the fix diff, from the commit the round before reviewed. A
/// retried opening keeps the commit it first recorded.
#[tokio::test]
async fn a_later_round_reviews_the_fix_diff() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string()), Some("bbb".to_string())].into();
    let rounds = rounds(&repo, &github);

    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("first");
    rounds
        .open_round(opening(run_id, 6, 2))
        .await
        .expect("second");
    rounds
        .open_round(opening(run_id, 6, 2))
        .await
        .expect("again");

    let recorded = repo.review_rounds(run_id).await.expect("rounds");
    let read: Vec<_> = recorded
        .iter()
        .map(|round| {
            (
                round.round,
                round.head_sha.as_deref().unwrap_or_default(),
                round.diff_base.as_deref().unwrap_or_default(),
                round.fix_diff,
            )
        })
        .collect();
    assert_eq!(read, [(1, "aaa", "main", false), (2, "bbb", "aaa", true)]);
}

/// A round after its entry was reset, as when a person sends the change
/// back at the merge gate, is the Run's next round, not round 1 again: it
/// is visit 1 of its entry, so the round limit counts from it, and it
/// reviews the whole change.
#[tokio::test]
async fn a_round_after_a_reset_is_the_run_s_next_round_over_the_whole_change() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [
        Some("aaa".to_string()),
        Some("bbb".to_string()),
        Some("ccc".to_string()),
    ]
    .into();
    let rounds = rounds(&repo, &github);

    for (seq, visit) in [(3, 1), (6, 2), (10, 1)] {
        rounds
            .open_round(opening(run_id, seq, visit))
            .await
            .expect("open");
    }

    let recorded = repo.review_rounds(run_id).await.expect("rounds");
    let read: Vec<_> = recorded
        .iter()
        .map(|round| {
            (
                round.round,
                round.visit,
                round.diff_base.as_deref().unwrap_or_default(),
                round.fix_diff,
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            (1, 1, "main", false),
            (2, 2, "aaa", true),
            (3, 1, "main", false)
        ]
    );
}

/// A fix that pushed nothing leaves its round nothing new to review: the
/// round reviews the whole change again, so what the fix left is found
/// again and blocks. A branch that is gone opens no round.
#[tokio::test]
async fn a_fix_that_changed_nothing_has_the_whole_change_reviewed_again() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    let rounds = rounds(&repo, &github);

    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("first");
    rounds
        .open_round(opening(run_id, 6, 2))
        .await
        .expect("second");
    let recorded = repo.review_rounds(run_id).await.expect("rounds");
    assert_eq!(
        (recorded[1].diff_base.as_deref(), recorded[1].fix_diff),
        (Some("main"), false)
    );
}

/// A round that cannot open, as when the Run's branch is gone, says why on
/// the Run and ends failed, and a retried opening says the same.
#[tokio::test]
async fn a_round_that_cannot_open_says_why() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [None].into();
    let rounds = rounds(&repo, &github);

    let opened = rounds.open_round(opening(run_id, 3, 1)).await;
    let again = rounds.open_round(opening(run_id, 3, 1)).await;

    assert!(matches!(opened, Ok(false)), "{opened:?}");
    assert!(matches!(again, Ok(false)), "{again:?}");
    let round = repo.review_rounds(run_id).await.expect("rounds").remove(0);
    assert_eq!(
        round.failure.as_deref(),
        Some("The run's branch is not on GitHub.")
    );
    assert_eq!(
        round.end.map(|end| end.outcome),
        Some(ReviewRoundOutcome::Failed)
    );
}

/// A finding in the diff the round reviewed blocks, and one outside it
/// becomes a follow-up task in the project of the Run's task, which holds
/// nothing. Closing again keeps the outcome and makes no second task.
#[tokio::test]
async fn a_finding_outside_the_reviewed_diff_becomes_a_follow_up_task() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    github.state().diffs.insert(
        ("main".to_string(), "aaa".to_string()),
        vec![ChangedFile::from_patch(
            "src/save.rs".to_string(),
            None,
            Some("@@ -1,3 +1,8 @@"),
        )],
    );
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(
        &repo,
        run_id,
        3,
        &["correctness-reviewer", "scope-reviewer"],
    )
    .await;
    let key = |seq| StepKey { run_id, seq };
    repo.record_findings(key(3), 3, &[finding("src/save.rs", Some((2, 4)))])
        .await
        .expect("record");
    repo.record_findings(key(4), 3, &[finding("src/load.rs", Some((7, 7)))])
        .await
        .expect("record");

    let routed = rounds.close_round(closing(run_id, 3)).await.expect("close");
    assert_eq!(routed, outcome("changes"));
    let again = rounds.close_round(closing(run_id, 3)).await.expect("again");
    assert_eq!(again, outcome("changes"));

    let round = repo.review_rounds(run_id).await.expect("rounds").remove(0);
    let kinds: Vec<_> = round
        .findings
        .iter()
        .map(|found| {
            (
                found.path.as_str(),
                found.kind,
                found.follow_up_task_id.is_some(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("src/save.rs", Some(FindingKind::Blocker), false),
            ("src/load.rs", Some(FindingKind::FollowUp), true),
        ]
    );
    let follow_ups = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(follow_ups.len(), 1);
    let made = &follow_ups[0];
    assert_eq!(made.owner, "macro|julia@example.com");
    assert_eq!(made.of, Some(TASK));
    assert_eq!(made.task.name, "Follow-up: src/load.rs drops the error");
    assert!(made.task.markdown.contains("`src/load.rs` line 7"));
    assert!(made.task.markdown.contains("the scope-reviewer"));
}

/// Findings all outside the reviewed diff hold nothing: the round passes.
/// A follow-up that could not become a task stays one, and still holds
/// nothing.
#[tokio::test]
async fn a_round_whose_findings_are_all_follow_ups_passes() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    github.state().diffs.insert(
        ("main".to_string(), "aaa".to_string()),
        vec![ChangedFile::from_patch(
            "src/save.rs".to_string(),
            None,
            None,
        )],
    );
    *repo.follow_ups_refused.lock().expect("refused") = true;
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 3, &["tests-reviewer"]).await;
    repo.record_findings(
        StepKey { run_id, seq: 3 },
        3,
        &[finding("docs/old.md", None)],
    )
    .await
    .expect("record");

    let routed = rounds.close_round(closing(run_id, 3)).await.expect("close");

    assert_eq!(routed, outcome("pass"));
    let round = repo.review_rounds(run_id).await.expect("rounds").remove(0);
    assert_eq!(round.findings[0].kind, Some(FindingKind::FollowUp));
    assert_eq!(round.findings[0].follow_up_task_id, None);
}

/// A diff GitHub cannot list lets no finding go: each blocks. A round
/// whose reviewer failed ends failed.
#[tokio::test]
async fn an_unlisted_diff_blocks_and_a_failed_reviewer_fails_the_round() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 3, &["tests-reviewer"]).await;
    repo.record_findings(
        StepKey { run_id, seq: 3 },
        3,
        &[finding("docs/old.md", None)],
    )
    .await
    .expect("record");

    let routed = rounds.close_round(closing(run_id, 3)).await.expect("close");
    assert_eq!(routed, outcome("changes"));
    assert!(repo.follow_ups.lock().expect("follow ups").is_empty());

    rounds
        .open_round(opening(run_id, 6, 2))
        .await
        .expect("open");
    let failed = ReviewRoundClosing {
        failed: true,
        ..closing(run_id, 6)
    };
    assert_eq!(
        rounds.close_round(failed).await.expect("close"),
        Outcome::failed()
    );
    let ended = repo.review_rounds(run_id).await.expect("rounds");
    assert_eq!(
        ended[1].end.as_ref().map(|end| end.outcome),
        Some(ReviewRoundOutcome::Failed)
    );
}

/// Hands back what a reviewer wrote, and says which round each Step read.
#[derive(Clone, Default)]
struct Reviewer {
    text: String,
    read: std::sync::Arc<std::sync::Mutex<Vec<Option<u16>>>>,
}

impl StepExecutor for Reviewer {
    type Err = StepFailure;

    fn serves(&self, _step: &StepName) -> Option<StepUse> {
        Some(StepUse::Agent)
    }

    async fn run(&self, _step: &StepName, input: StepInput<'_>) -> Result<StepReport, StepFailure> {
        self.read
            .lock()
            .expect("read")
            .push(input.review.map(|round| round.seq));
        Ok(StepReport {
            outcome: outcome("changes"),
            output: self.text.clone(),
        })
    }

    async fn stop(
        &self,
        _step: &StepName,
        _role: Option<&Role>,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), StepFailure> {
        Ok(())
    }

    async fn release(
        &self,
        _step: &StepName,
        _role: Option<&Role>,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), StepFailure> {
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the step failed")]
struct StepFailure;

impl crate::domain::ports::Retryable for StepFailure {
    fn retryable(&self) -> bool {
        false
    }
}

impl crate::domain::ports::AgentFailure for StepFailure {}

/// A reviewer's Step reads the round it reviews in, and a Step after the
/// round reads it once it ended, for its Blockers.
#[tokio::test]
async fn a_step_reads_its_round_or_the_latest_that_ended() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    let entry = crate::domain::flow::Entry {
        id: EntryId::try_from("review".to_string()).expect("entry"),
        step: StepName::try_from("review".to_string()).expect("step"),
        role: None,
        reviewers: vec![role("correctness-reviewer")],
        when: Vec::new(),
        max_visits: NonZeroU8::new(3),
        on_exhausted: Vec::new(),
        next: None,
        on: Default::default(),
        gate: None,
    };
    let key = StepKey { run_id, seq: 3 };
    let request = StepRequest::reviewer(key, &entry, 1, &role("correctness-reviewer"), 3);
    let reviewer = Reviewer::default();
    let steps = StepServiceImpl::new(repo.clone(), reviewer.clone());
    steps.start_step(request.clone()).await.expect("start");
    steps.run_step(request).await.expect("run");
    repo.record_findings(key, 3, &[]).await.expect("record");

    let fix = StepRequest {
        key: StepKey { run_id, seq: 4 },
        entry: EntryId::try_from("fix".to_string()).expect("entry"),
        step: StepName::try_from("fix".to_string()).expect("step"),
        role: Some(role("fixer")),
        visit: 1,
        round: None,
        may_ask: false,
        questions_left: 0,
        resumes: None,
    };
    steps.start_step(fix.clone()).await.expect("start");
    steps.run_step(fix.clone()).await.expect("run");
    rounds.close_round(closing(run_id, 3)).await.expect("close");
    steps.run_step(fix).await.expect("run again");

    assert_eq!(
        *reviewer.read.lock().expect("read"),
        [Some(3), None, Some(3)]
    );
}

/// Workflows that are always there: the Runs here are read, never started.
struct Started;

impl crate::domain::ports::RunWorkflows for Started {
    async fn start(
        &self,
        _start: &crate::domain::models::RunStart,
    ) -> Result<(), crate::domain::ports::RunWorkflowsError> {
        Ok(())
    }

    async fn exists(
        &self,
        _run_id: RunId,
    ) -> Result<bool, crate::domain::ports::RunWorkflowsError> {
        Ok(true)
    }

    async fn answer(
        &self,
        _run_id: RunId,
        _answer: &crate::domain::gate::AnswerGate,
    ) -> Result<crate::domain::ports::AnswerDelivery, crate::domain::ports::RunWorkflowsError> {
        Ok(crate::domain::ports::AnswerDelivery::Taken)
    }

    async fn reply(
        &self,
        _run_id: RunId,
        _reply: &crate::domain::gate::ReplyGate,
    ) -> Result<crate::domain::ports::AnswerDelivery, crate::domain::ports::RunWorkflowsError> {
        Ok(crate::domain::ports::AnswerDelivery::Taken)
    }

    async fn cancel(
        &self,
        _run_id: RunId,
    ) -> Result<bool, crate::domain::ports::RunWorkflowsError> {
        Ok(true)
    }
}

/// A Run read alone, and the Runs of its task, show each review round with
/// what its reviewers found.
#[tokio::test]
async fn a_run_shows_its_review_rounds_and_their_findings() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    rounds(&repo, &github)
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 3, &["tests-reviewer"]).await;
    repo.record_findings(
        StepKey { run_id, seq: 3 },
        3,
        &[finding("src/save.rs", None)],
    )
    .await
    .expect("record");
    let service = crate::domain::service::FactoryServiceImpl::new(
        repo.clone(),
        Started,
        Reviewer::default(),
        repo.clone(),
        repo.clone(),
        github.clone(),
    );

    let read = crate::domain::ports::FactoryService::get_run(&service, &julia(), run_id)
        .await
        .expect("read");
    let listed = crate::domain::ports::FactoryService::list_task_runs(&service, &julia(), TASK)
        .await
        .expect("listed");

    assert_eq!(read.review_rounds.len(), 1);
    assert_eq!(read.review_rounds[0].findings[0].path, "src/save.rs");
    assert_eq!(
        read.review_rounds[0].findings[0].role,
        Some(role("tests-reviewer"))
    );
    assert_eq!(listed[0].review_rounds, read.review_rounds);
}

/// A finding that blocks wherever it is.
fn found_again(path: &str) -> Finding {
    Finding {
        blocks_anywhere: true,
        ..finding(path, Some((5, 5)))
    }
}

/// Round 1 finds A, B and C in the change; the fix addresses A only, so
/// its diff touches only A's file. Round 2's reviewers check the Blockers
/// of round 1 again and find B and C still there, outside the fix diff:
/// they still block, and the round goes to the fix again. A new finding
/// outside the fix diff is a follow-up as before.
#[tokio::test]
async fn blockers_a_partial_fix_left_still_block_the_next_round() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string()), Some("bbb".to_string())].into();
    github.state().diffs.insert(
        ("main".to_string(), "aaa".to_string()),
        ["a.rs", "b.rs", "c.rs"]
            .map(|path| ChangedFile::from_patch(path.to_string(), None, None))
            .to_vec(),
    );
    github.state().diffs.insert(
        ("aaa".to_string(), "bbb".to_string()),
        vec![ChangedFile::from_patch("a.rs".to_string(), None, None)],
    );
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 3, &["correctness-reviewer"]).await;
    let found = ["a.rs", "b.rs", "c.rs"].map(|path| finding(path, Some((5, 5))));
    repo.record_findings(StepKey { run_id, seq: 3 }, 3, &found)
        .await
        .expect("record");
    assert_eq!(
        rounds.close_round(closing(run_id, 3)).await.expect("close"),
        outcome("changes")
    );

    rounds
        .open_round(opening(run_id, 5, 2))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 5, &["correctness-reviewer"]).await;
    let all = repo.review_rounds(run_id).await.expect("rounds");
    let carried: Vec<&str> = super::carried_blockers(&all, &all[1])
        .iter()
        .map(|finding| finding.path.as_str())
        .collect();
    assert_eq!(carried, ["a.rs", "b.rs", "c.rs"]);
    let again = [
        found_again("b.rs"),
        found_again("c.rs"),
        finding("d.rs", None),
    ];
    repo.record_findings(StepKey { run_id, seq: 5 }, 5, &again)
        .await
        .expect("record");

    let routed = rounds.close_round(closing(run_id, 5)).await.expect("close");

    assert_eq!(routed, outcome("changes"));
    let round = repo.review_rounds(run_id).await.expect("rounds").remove(1);
    let kinds: Vec<_> = round
        .findings
        .iter()
        .map(|finding| (finding.path.as_str(), finding.kind))
        .collect();
    assert_eq!(
        kinds,
        [
            ("b.rs", Some(FindingKind::Blocker)),
            ("c.rs", Some(FindingKind::Blocker)),
            ("d.rs", Some(FindingKind::FollowUp)),
        ]
    );
}

/// The last round that still finds Blockers ends the Run for a person
/// (`unresolved`) rather than go to a fix no round would review.
#[tokio::test]
async fn the_last_round_s_blockers_are_left_unresolved() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 3, &["tests-reviewer"]).await;
    repo.record_findings(StepKey { run_id, seq: 3 }, 3, &[finding("a.rs", None)])
        .await
        .expect("record");
    let last = ReviewRoundClosing {
        last: true,
        ..closing(run_id, 3)
    };

    assert_eq!(
        rounds.close_round(last).await.expect("close"),
        outcome(super::UNRESOLVED)
    );
    let round = repo.review_rounds(run_id).await.expect("rounds").remove(0);
    assert_eq!(
        round.end.map(|end| end.outcome),
        Some(ReviewRoundOutcome::Changes)
    );
}

/// A reviewer's Step that ended with no answer read, as one a worker
/// without review panels ran, reviewed nothing: the round fails rather
/// than pass on it.
#[tokio::test]
async fn a_reviewer_that_ended_with_no_answer_read_fails_the_round() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 3, &["tests-reviewer", "scope-reviewer"]).await;
    repo.record_findings(StepKey { run_id, seq: 3 }, 3, &[])
        .await
        .expect("record");
    for seq in [3, 4] {
        repo.end_step(StepKey { run_id, seq }, &outcome("pass"), "Looks fine")
            .await
            .expect("end");
    }

    let routed = rounds.close_round(closing(run_id, 3)).await.expect("close");

    assert_eq!(routed, Outcome::failed());
}

/// A follow-up task whose making fails as an outage would fails the close,
/// for Temporal to try again; one made though its answer was lost is found
/// by its id on the retry, not made twice.
#[tokio::test]
async fn a_retried_close_makes_each_follow_up_task_once() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    github
        .state()
        .diffs
        .insert(("main".to_string(), "aaa".to_string()), Vec::new());
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(&repo, run_id, 3, &["tests-reviewer"]).await;
    repo.record_findings(
        StepKey { run_id, seq: 3 },
        3,
        &[finding("docs/old.md", None)],
    )
    .await
    .expect("record");
    *repo.follow_ups_fail.lock().expect("fail") = 1;
    *repo.follow_ups_made_anyway.lock().expect("anyway") = true;

    let failed = rounds.close_round(closing(run_id, 3)).await;
    assert!(matches!(failed, Err(StepError::Internal(_))), "{failed:?}");
    let retried = rounds.close_round(closing(run_id, 3)).await.expect("retry");

    assert_eq!(retried, outcome("pass"));
    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 1);
    let round = repo.review_rounds(run_id).await.expect("rounds").remove(0);
    assert_eq!(round.findings[0].follow_up_task_id, Some(made[0].id));
}

/// Two reviewers that report one problem outside the diff, at lines of the
/// same file that meet, make one follow-up task naming both, and each
/// finding stays recorded, linked to it; a problem elsewhere, and two of
/// one reviewer's own findings, each make a task of their own. A retried
/// close makes none again.
#[tokio::test]
async fn reviewers_reporting_one_problem_make_one_follow_up_task() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    github
        .state()
        .diffs
        .insert(("main".to_string(), "aaa".to_string()), Vec::new());
    let rounds = rounds(&repo, &github);
    rounds
        .open_round(opening(run_id, 3, 1))
        .await
        .expect("open");
    start_reviewers(
        &repo,
        run_id,
        3,
        &["correctness-reviewer", "scope-reviewer"],
    )
    .await;
    let key = |seq| StepKey { run_id, seq };
    repo.record_findings(
        key(3),
        3,
        &[
            finding("src/text.js", Some((6, 15))),
            finding("src/text.js", Some((30, 31))),
        ],
    )
    .await
    .expect("record");
    repo.record_findings(
        key(4),
        3,
        &[
            finding("./src/text.js", Some((5, 16))),
            finding("src/other.js", Some((6, 15))),
        ],
    )
    .await
    .expect("record");

    let closed = rounds.close_round(closing(run_id, 3)).await.expect("close");
    assert_eq!(closed, outcome("pass"));
    let again = rounds.close_round(closing(run_id, 3)).await.expect("again");
    assert_eq!(again, outcome("pass"));

    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 3, "{made:#?}");
    let round = repo.review_rounds(run_id).await.expect("rounds").remove(0);
    let linked: Vec<_> = round
        .findings
        .iter()
        .map(|found| (found.seq, found.place, found.follow_up_task_id))
        .collect();
    let task = |index: usize| Some(made[index].id);
    assert_eq!(
        linked,
        [
            (3, 1, task(0)),
            (3, 2, task(1)),
            (4, 1, task(0)),
            (4, 2, task(2)),
        ]
    );
    let shared = &made[0].task.markdown;
    assert!(
        shared.starts_with("the correctness-reviewer and the scope-reviewer found this"),
        "{shared}"
    );
    assert!(
        shared.contains("## Reported by the correctness-reviewer")
            && shared.contains("## Reported by the scope-reviewer"),
        "{shared}"
    );
    assert!(shared.contains("`src/text.js` lines 6"), "{shared}");
    assert!(shared.contains("`./src/text.js` lines 5"), "{shared}");
    assert!(
        made[2]
            .task
            .markdown
            .starts_with("the scope-reviewer found this"),
        "{}",
        made[2].task.markdown
    );
}

/// Open the round at `seq`, the Run's `round`th visit of its entry, start
/// its reviewers of `roles`, record what each found, and close it.
async fn review(
    rounds: &ReviewRoundsImpl<FakeRepo, FakeGithub, FakeRepo>,
    repo: &FakeRepo,
    run_id: RunId,
    (seq, visit): (u16, u16),
    found: &[(&str, Vec<Finding>)],
) -> Result<Outcome, StepError> {
    rounds
        .open_round(opening(run_id, seq, visit))
        .await
        .expect("open");
    let roles: Vec<&str> = found.iter().map(|(role, _)| *role).collect();
    start_reviewers(repo, run_id, seq, &roles).await;
    for (offset, (_, findings)) in (0..).zip(found) {
        let key = StepKey {
            run_id,
            seq: seq + offset,
        };
        repo.record_findings(key, seq, findings)
            .await
            .expect("record");
    }
    rounds.close_round(closing(run_id, seq)).await
}

/// The task each follow-up of the Run is linked to, by its Step and place.
async fn linked(repo: &FakeRepo, run_id: RunId) -> Vec<(u16, u16, Option<Uuid>)> {
    repo.review_rounds(run_id)
        .await
        .expect("rounds")
        .iter()
        .flat_map(|round| &round.findings)
        .map(|found| (found.seq, found.place, found.follow_up_task_id))
        .collect()
}

/// A problem outside the diff that later rounds report again makes no
/// second task: its findings are linked to the first, which a comment of
/// the owner's tells, once a round, who reported it again and where. A
/// finding elsewhere in the same file makes a task of its own. A finding
/// about the whole file meets both tasks, the newer one last linked: the
/// oldest wins.
#[tokio::test]
async fn a_problem_reported_again_in_a_later_round_joins_its_follow_up_task() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [
        Some("aaa".to_string()),
        Some("bbb".to_string()),
        Some("ccc".to_string()),
    ]
    .into();
    for (base, head) in [("main", "aaa"), ("aaa", "bbb"), ("bbb", "ccc")] {
        github
            .state()
            .diffs
            .insert((base.to_string(), head.to_string()), Vec::new());
    }
    let rounds = rounds(&repo, &github);

    let first = review(
        &rounds,
        &repo,
        run_id,
        (3, 1),
        &[(
            "correctness-reviewer",
            vec![finding("src/text.js", Some((6, 15)))],
        )],
    )
    .await;
    assert_eq!(first.expect("first"), outcome("pass"));
    let second = review(
        &rounds,
        &repo,
        run_id,
        (6, 2),
        &[
            (
                "scope-reviewer",
                vec![finding("./src/text.js", Some((14, 20)))],
            ),
            (
                "correctness-reviewer",
                vec![finding("src/text.js", Some((40, 41)))],
            ),
        ],
    )
    .await;
    assert_eq!(second.expect("second"), outcome("pass"));
    let third = review(
        &rounds,
        &repo,
        run_id,
        (9, 3),
        &[
            ("correctness-reviewer", vec![finding("src/text.js", None)]),
            ("scope-reviewer", vec![finding("src/text.js", Some((8, 8)))]),
        ],
    )
    .await;
    assert_eq!(third.expect("third"), outcome("pass"));

    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 2, "{made:#?}");
    let task = |index: usize| Some(made[index].id);
    assert_eq!(
        linked(&repo, run_id).await,
        [
            (3, 1, task(0)),
            (6, 1, task(0)),
            (7, 1, task(1)),
            (9, 1, task(0)),
            (10, 1, task(0)),
        ]
    );
    let comments = repo.follow_up_comments.lock().expect("comments").clone();
    let read: Vec<_> = comments
        .iter()
        .map(|comment| {
            (
                comment.owner.as_str(),
                comment.task_id,
                comment.text.as_str(),
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            (
                "macro|julia@example.com",
                made[0].id,
                "Reported again in review round 2 by the scope-reviewer, at `src/text.js` \
                 lines 14-20."
            ),
            (
                "macro|julia@example.com",
                made[0].id,
                "Reported again in review round 3 by the correctness-reviewer and the \
                 scope-reviewer, at `src/text.js`, `src/text.js` line 8."
            ),
        ]
    );
}

/// A round after a revise reset its entry is still the Run's: a problem a
/// round before the revise made a task of joins that task.
#[tokio::test]
async fn a_problem_reported_again_after_a_revise_joins_its_follow_up_task() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string()), Some("bbb".to_string())].into();
    for (base, head) in [("main", "aaa"), ("main", "bbb")] {
        github
            .state()
            .diffs
            .insert((base.to_string(), head.to_string()), Vec::new());
    }
    let rounds = rounds(&repo, &github);

    // A reviewer may write what the task's own text says.
    let echoing = Finding {
        problem: "docs/old.md is outside the changes it reviewed, yet stale".to_string(),
        ..finding("docs/old.md", None)
    };
    review(
        &rounds,
        &repo,
        run_id,
        (3, 1),
        &[("correctness-reviewer", vec![echoing])],
    )
    .await
    .expect("before the revise");
    review(
        &rounds,
        &repo,
        run_id,
        (10, 1),
        &[("tests-reviewer", vec![finding("docs/old.md", Some((3, 4)))])],
    )
    .await
    .expect("after the revise");

    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 1, "{made:#?}");
    assert_eq!(
        linked(&repo, run_id).await,
        [(3, 1, Some(made[0].id)), (10, 1, Some(made[0].id))]
    );
    let comments = repo.follow_up_comments.lock().expect("comments").clone();
    assert_eq!(comments.len(), 1, "{comments:#?}");
    assert_eq!(
        comments[0].text,
        "Reported again in review round 2 by the tests-reviewer, at `docs/old.md` lines 3-4."
    );
}

/// A close that fails after a problem reported again joined its task, as
/// when another follow-up's task cannot be made yet, is tried again: the
/// retry makes the new task once and comments on the joined one no more.
#[tokio::test]
async fn a_retried_close_comments_on_a_joined_follow_up_task_once() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string()), Some("bbb".to_string())].into();
    for (base, head) in [("main", "aaa"), ("aaa", "bbb")] {
        github
            .state()
            .diffs
            .insert((base.to_string(), head.to_string()), Vec::new());
    }
    let rounds = rounds(&repo, &github);
    review(
        &rounds,
        &repo,
        run_id,
        (3, 1),
        &[(
            "correctness-reviewer",
            vec![finding("src/text.js", Some((6, 15)))],
        )],
    )
    .await
    .expect("first");

    *repo.follow_ups_fail.lock().expect("fail") = 1;
    let failed = review(
        &rounds,
        &repo,
        run_id,
        (6, 2),
        &[(
            "correctness-reviewer",
            vec![
                finding("src/text.js", Some((10, 10))),
                finding("src/other.js", Some((1, 2))),
            ],
        )],
    )
    .await;
    assert!(matches!(failed, Err(StepError::Internal(_))), "{failed:?}");
    let retried = rounds.close_round(closing(run_id, 6)).await.expect("retry");
    assert_eq!(retried, outcome("pass"));

    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 2, "{made:#?}");
    assert_eq!(
        linked(&repo, run_id).await,
        [
            (3, 1, Some(made[0].id)),
            (6, 1, Some(made[0].id)),
            (6, 2, Some(made[1].id)),
        ]
    );
    let comments = repo.follow_up_comments.lock().expect("comments").clone();
    assert_eq!(comments.len(), 1, "{comments:#?}");
}

/// A Run whose rounds at Steps 3 and 6 review `main..aaa` and `aaa..bbb`,
/// and whose first round made one follow-up task, on `src/text.js` lines
/// 6-15: the rounds, and the task's id.
async fn reported_once(
    repo: &FakeRepo,
    github: &FakeGithub,
) -> (
    ReviewRoundsImpl<FakeRepo, FakeGithub, FakeRepo>,
    RunId,
    Uuid,
) {
    let run_id = code_run(repo).await;
    github.state().branch_heads = [Some("aaa".to_string()), Some("bbb".to_string())].into();
    for (base, head) in [("main", "aaa"), ("aaa", "bbb")] {
        github
            .state()
            .diffs
            .insert((base.to_string(), head.to_string()), Vec::new());
    }
    let rounds = rounds(repo, github);
    review(
        &rounds,
        repo,
        run_id,
        (3, 1),
        &[(
            "correctness-reviewer",
            vec![finding("src/text.js", Some((6, 15)))],
        )],
    )
    .await
    .expect("first");
    let made = repo.follow_ups.lock().expect("follow ups")[0].id;
    (rounds, run_id, made)
}

/// A task the owner deleted since is not joined: the problem reported again
/// makes a task of its own.
#[tokio::test]
async fn a_problem_whose_task_is_gone_makes_a_new_follow_up_task() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let (rounds, run_id, gone) = reported_once(&repo, &github).await;
    repo.tasks.lock().expect("tasks").remove(&gone);

    review(
        &rounds,
        &repo,
        run_id,
        (6, 2),
        &[("scope-reviewer", vec![finding("src/text.js", Some((7, 7)))])],
    )
    .await
    .expect("second");

    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 2, "{made:#?}");
    assert_eq!(
        linked(&repo, run_id).await,
        [(3, 1, Some(gone)), (6, 1, Some(made[1].id))]
    );
    assert!(repo.follow_up_comments.lock().expect("comments").is_empty());
}

/// A follow-up document storage refused has no task to join: the problem
/// reported again makes one.
#[tokio::test]
async fn a_problem_whose_task_was_refused_makes_a_follow_up_task_later() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    *repo.follow_ups_refused.lock().expect("refused") = true;
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string()), Some("bbb".to_string())].into();
    for (base, head) in [("main", "aaa"), ("aaa", "bbb")] {
        github
            .state()
            .diffs
            .insert((base.to_string(), head.to_string()), Vec::new());
    }
    let rounds = rounds(&repo, &github);
    review(
        &rounds,
        &repo,
        run_id,
        (3, 1),
        &[(
            "correctness-reviewer",
            vec![finding("src/text.js", Some((6, 15)))],
        )],
    )
    .await
    .expect("refused");
    *repo.follow_ups_refused.lock().expect("refused") = false;

    review(
        &rounds,
        &repo,
        run_id,
        (6, 2),
        &[("scope-reviewer", vec![finding("src/text.js", Some((7, 7)))])],
    )
    .await
    .expect("second");

    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 1, "{made:#?}");
    assert_eq!(
        linked(&repo, run_id).await,
        [(3, 1, None), (6, 1, Some(made[0].id))]
    );
    assert!(repo.follow_up_comments.lock().expect("comments").is_empty());
}

/// Every finding of a round that joins one task, across its problems, is
/// linked before the task is commented on, once, naming them all: a link
/// that fails fails the close with no comment made, and the retry makes
/// the one comment.
#[tokio::test]
async fn a_round_links_what_joined_a_task_then_comments_on_it_once() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let (rounds, run_id, task) = reported_once(&repo, &github).await;
    repo.fail_once_after("link_follow_up", 1);

    // Two findings of one reviewer that meet are two problems, as within a
    // round, and line 12 meets neither: three problems, all the earlier
    // task's.
    let failed = review(
        &rounds,
        &repo,
        run_id,
        (6, 2),
        &[
            (
                "correctness-reviewer",
                vec![
                    finding("src/text.js", Some((7, 8))),
                    finding("src/text.js", Some((9, 9))),
                ],
            ),
            (
                "scope-reviewer",
                vec![finding("/src/text.js", Some((12, 12)))],
            ),
        ],
    )
    .await;
    assert!(matches!(failed, Err(StepError::Internal(_))), "{failed:?}");
    assert!(repo.follow_up_comments.lock().expect("comments").is_empty());
    let retried = rounds.close_round(closing(run_id, 6)).await.expect("retry");
    assert_eq!(retried, outcome("pass"));

    assert_eq!(repo.follow_ups.lock().expect("follow ups").len(), 1);
    assert_eq!(
        linked(&repo, run_id).await,
        [
            (3, 1, Some(task)),
            (6, 1, Some(task)),
            (6, 2, Some(task)),
            (7, 1, Some(task)),
        ]
    );
    let comments = repo.follow_up_comments.lock().expect("comments").clone();
    let read: Vec<_> = comments
        .iter()
        .map(|comment| (comment.task_id, comment.text.as_str()))
        .collect();
    assert_eq!(
        read,
        [(
            task,
            "Reported again in review round 2 by the correctness-reviewer and the \
             scope-reviewer, at `src/text.js` lines 7-8, `src/text.js` line 9, \
             `src/text.js` line 12."
        )]
    );
}

/// A comment that fails is logged and holds nothing; a close tried again
/// for another reason, as a new task that could not be made yet, makes it.
#[tokio::test]
async fn a_comment_that_failed_is_made_when_the_close_is_tried_again() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let (rounds, run_id, task) = reported_once(&repo, &github).await;
    repo.fail_once_after("comment_on_follow_up", 0);
    *repo.follow_ups_fail.lock().expect("fail") = 1;

    let failed = review(
        &rounds,
        &repo,
        run_id,
        (6, 2),
        &[(
            "scope-reviewer",
            vec![
                finding("src/text.js", Some((7, 7))),
                finding("src/other.js", None),
            ],
        )],
    )
    .await;
    assert!(matches!(failed, Err(StepError::Internal(_))), "{failed:?}");
    assert!(repo.follow_up_comments.lock().expect("comments").is_empty());
    let retried = rounds.close_round(closing(run_id, 6)).await.expect("retry");
    assert_eq!(retried, outcome("pass"));

    let comments = repo.follow_up_comments.lock().expect("comments").clone();
    assert_eq!(comments.len(), 1, "{comments:#?}");
    assert_eq!(comments[0].task_id, task);
    assert_eq!(repo.follow_ups.lock().expect("follow ups").len(), 2);
}

/// A close tried again after it made one of its round's tasks does not
/// take that task for an earlier round's: it is not commented on.
#[tokio::test]
async fn a_retried_close_does_not_join_a_task_its_own_round_made() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    let run_id = code_run(&repo).await;
    github.state().branch_heads = [Some("aaa".to_string())].into();
    github
        .state()
        .diffs
        .insert(("main".to_string(), "aaa".to_string()), Vec::new());
    let rounds = rounds(&repo, &github);
    repo.fail_once_after("create_follow_up", 1);

    let failed = review(
        &rounds,
        &repo,
        run_id,
        (3, 1),
        &[(
            "correctness-reviewer",
            vec![
                finding("src/text.js", Some((1, 5))),
                finding("src/text.js", Some((3, 4))),
            ],
        )],
    )
    .await;
    assert!(matches!(failed, Err(StepError::Internal(_))), "{failed:?}");
    let retried = rounds.close_round(closing(run_id, 3)).await.expect("retry");
    assert_eq!(retried, outcome("pass"));

    let made = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(made.len(), 2, "{made:#?}");
    assert!(repo.follow_up_comments.lock().expect("comments").is_empty());
}
