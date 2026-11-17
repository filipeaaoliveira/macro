use std::num::NonZeroU8;

use factory_records::domain::models::{
    EntryId, Outcome, Repository, Role, RunId, RunInsert, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    Finding, FindingKind, Lines, ReviewRoundOutcome, ReviewRoundRepository,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{
    ChangedFile, ReviewRoundClosing, ReviewRoundOpening, ReviewRoundsImpl, ReviewerAnswer,
    ReviewerAnswerError, in_diff,
};
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
        .insert_run(
            id,
            "code",
            "Add dark mode",
            &julia(),
            Some(TASK),
            Some(&repository),
        )
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

#[test]
fn a_reviewer_hands_back_its_blockers_in_a_json_block() {
    let text = "Checked the save path.\n\n```json\n{\"blockers\": [\
        {\"file\": \"./src/save.rs\", \"startLine\": 12, \"problem\": \"Drops the error\", \"why\": \"Saves look done\"},\
        {\"file\": \"README.md\", \"problem\": \"Out of date\", \"why\": \"It names the old flag\"}\
    ]}\n```\n";

    let answer = ReviewerAnswer::read(&outcome("changes"), text).expect("reads");

    assert_eq!(answer.summary, "Checked the save path.");
    assert_eq!(answer.findings[0].path, "src/save.rs");
    assert_eq!(answer.findings[0].lines, Some(Lines { start: 12, end: 12 }));
    assert_eq!(answer.findings[1].lines, None);
    let rendered = answer.render();
    assert!(rendered.starts_with("Checked the save path.\n\n### Blockers\n"));
    assert!(
        rendered.contains("1. `src/save.rs` line 12: Drops the error\n   Why: Saves look done")
    );
}

#[test]
fn a_hand_back_whose_blockers_do_not_read_or_fit_its_outcome_is_refused() {
    let none = "```json\n{\"blockers\": []}\n```";
    let one =
        "```json\n{\"blockers\": [{\"file\": \"a.rs\", \"problem\": \"p\", \"why\": \"w\"}]}\n```";
    let backwards = "```json\n{\"blockers\": [{\"file\": \"a.rs\", \"startLine\": 9, \"endLine\": 3, \"problem\": \"p\", \"why\": \"w\"}]}\n```";
    let no_why =
        "```json\n{\"blockers\": [{\"file\": \"a.rs\", \"problem\": \"p\", \"why\": \" \"}]}\n```";

    assert!(ReviewerAnswer::read(&outcome("pass"), none).is_ok());
    assert_eq!(
        ReviewerAnswer::read(&outcome("pass"), one),
        Err(ReviewerAnswerError::OutcomeAndBlockers)
    );
    assert_eq!(
        ReviewerAnswer::read(&outcome("changes"), none),
        Err(ReviewerAnswerError::OutcomeAndBlockers)
    );
    assert!(matches!(
        ReviewerAnswer::read(&outcome("changes"), "Looks wrong to me."),
        Err(ReviewerAnswerError::NoBlockers(_))
    ));
    assert_eq!(
        ReviewerAnswer::read(&outcome("changes"), backwards),
        Err(ReviewerAnswerError::InvalidBlocker(1))
    );
    assert_eq!(
        ReviewerAnswer::read(&outcome("changes"), no_why),
        Err(ReviewerAnswerError::InvalidBlocker(1))
    );
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
                round.head_sha.as_str(),
                round.diff_base.as_str(),
                round.fix_diff,
            )
        })
        .collect();
    assert_eq!(read, [(1, "aaa", "main", false), (2, "bbb", "aaa", true)]);
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
        (recorded[1].diff_base.as_str(), recorded[1].fix_diff),
        ("main", false)
    );

    github.state().branch_heads = [None].into();
    let gone = rounds.open_round(opening(run_id, 9, 3)).await;
    assert!(matches!(gone, Err(StepError::Final(_))), "{gone:?}");
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
    *repo.follow_ups_fail.lock().expect("fail") = true;
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

/// A reviewer's Step reads its round, records its Blockers on it, and ends
/// with them written out; one whose hand back does not read fails for
/// good. A Step after the round reads it once it ended, for its Blockers.
#[tokio::test]
async fn a_reviewer_s_blockers_are_recorded_on_its_round() {
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
    let reviewer = Reviewer {
        text: "Read it.\n```json\n{\"blockers\": [{\"file\": \"src/save.rs\", \"startLine\": 2, \"endLine\": 4, \"problem\": \"Drops the error\", \"why\": \"Saves look done\"}]}\n```".to_string(),
        ..Reviewer::default()
    };
    let steps = StepServiceImpl::new(repo.clone(), reviewer.clone());
    steps.start_step(request.clone()).await.expect("start");

    let report = steps.run_step(request.clone()).await.expect("run");

    assert_eq!(report.outcome, outcome("changes"));
    assert!(report.output.contains("### Blockers"));
    assert!(!report.output.contains("```json"));
    let round = repo.review_rounds(run_id).await.expect("rounds").remove(0);
    assert_eq!(round.findings.len(), 1);
    assert_eq!(round.findings[0].lines, Some(Lines { start: 2, end: 4 }));

    let garbled = Reviewer {
        text: "No blocks here.".to_string(),
        ..Reviewer::default()
    };
    let failed = StepServiceImpl::new(repo.clone(), garbled)
        .run_step(request)
        .await;
    assert!(matches!(failed, Err(StepError::Final(_))), "{failed:?}");

    // The fix after the round reads it once it ended.
    rounds.close_round(closing(run_id, 3)).await.expect("close");
    let fix = StepRequest {
        key: StepKey { run_id, seq: 4 },
        entry: EntryId::try_from("fix".to_string()).expect("entry"),
        step: StepName::try_from("fix".to_string()).expect("step"),
        role: Some(role("fixer")),
        visit: 1,
        round: None,
    };
    steps.start_step(fix.clone()).await.expect("start");
    steps.run_step(fix).await.ok();
    assert_eq!(*reviewer.read.lock().expect("read"), [Some(3), Some(3)]);
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

    async fn exists(&self, _run_id: RunId) -> Result<bool, crate::domain::ports::RunWorkflowsError> {
        Ok(true)
    }

    async fn answer(
        &self,
        _run_id: RunId,
        _answer: &crate::domain::gate::AnswerGate,
    ) -> Result<crate::domain::ports::AnswerDelivery, crate::domain::ports::RunWorkflowsError> {
        Ok(crate::domain::ports::AnswerDelivery::Taken)
    }

    async fn cancel(&self, _run_id: RunId) -> Result<bool, crate::domain::ports::RunWorkflowsError> {
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
    repo.record_findings(StepKey { run_id, seq: 3 }, 3, &[finding("src/save.rs", None)])
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
