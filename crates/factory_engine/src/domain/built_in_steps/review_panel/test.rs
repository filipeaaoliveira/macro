//! A reviewer's Step of a review round, with an agent the test scripts.

mod dispute;
mod session;

use factory_records::domain::standing_rules::RunRules;
use std::sync::{Arc, LazyLock, Mutex};

use factory_records::domain::models::{
    EntryId, NewRun, Outcome, Repository, Role, RunId, RunInsert, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{
    Finding, FindingClass, FindingKind, ReviewRound, ReviewRoundOutcome, ReviewRoundRepository,
    ReviewRoundStart,
};
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::fakes::{FakeGithub, FakeRepo, FakeWatches};
use crate::domain::flow::StepName;
use crate::domain::models::{AgentAnswer, AgentTask, AgentTurn, StepInput};
use crate::domain::ports::{AgentFailure, Retryable, StepAgent, StepExecutor};
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::review_rounds::WHOLE_CHANGE;

#[derive(Debug, thiserror::Error)]
#[error("the agent failed")]
struct AgentFailed;

impl Retryable for AgentFailed {
    fn retryable(&self) -> bool {
        false
    }
}

impl AgentFailure for AgentFailed {}

/// Hands back `first` when asked for its review, and `again` when asked
/// once more, keeping each prompt and later prompt; fails the first
/// `turns_failing` later prompts, as a lost attempt would.
#[derive(Clone)]
struct Reviewer {
    first: (&'static str, String),
    again: (&'static str, String),
    tasks: Arc<Mutex<Vec<AgentTask>>>,
    turns: Arc<Mutex<Vec<AgentTurn>>>,
    turns_failing: Arc<Mutex<usize>>,
}

impl Reviewer {
    fn answering(first: (&'static str, &str), again: (&'static str, &str)) -> Self {
        Self {
            first: (first.0, first.1.to_string()),
            again: (again.0, again.1.to_string()),
            tasks: Arc::default(),
            turns: Arc::default(),
            turns_failing: Arc::default(),
        }
    }
}

fn answer((outcome, text): &(&'static str, String)) -> AgentAnswer {
    AgentAnswer {
        outcome: Outcome::try_from(outcome.to_string()).expect("valid"),
        text: text.clone(),
    }
}

impl StepAgent for Reviewer {
    type Err = AgentFailed;

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, AgentFailed> {
        self.tasks.lock().expect("tasks").push(task);
        Ok(answer(&self.first))
    }

    async fn follow_up(&self, turn: AgentTurn) -> Result<AgentAnswer, AgentFailed> {
        self.turns.lock().expect("turns").push(turn);
        let mut failing = self.turns_failing.lock().expect("failing");
        if *failing > 0 {
            *failing -= 1;
            return Err(AgentFailed);
        }
        Ok(answer(&self.again))
    }

    async fn run_check(
        &self,
        _command: crate::domain::models::AgentCommand,
    ) -> Result<Option<agent_session::domain::session_command::SessionCommandOutput>, AgentFailed>
    {
        Ok(None)
    }

    async fn stop(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), AgentFailed> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), AgentFailed> {
        Ok(())
    }
}

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

static REPOSITORY: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/app".to_string(),
    base_branch: "trunk".to_string(),
});

static CORRECTNESS: LazyLock<Role> =
    LazyLock::new(|| Role::try_from("correctness-reviewer".to_string()).expect("valid"));

const PASSES: &str = "Checked it.\n```json\n{\"blockers\": []}\n```";

/// A Run with round 1 at Step 3, which found `blockers` and ended, and
/// round 2 at Step 5, `fix_diff` or not, whose first reviewer's Step is
/// started: the round 2 recorded.
async fn second_round(repo: &FakeRepo, blockers: &[&str], fix_diff: bool) -> (RunId, ReviewRound) {
    let found: Vec<Finding> = blockers
        .iter()
        .map(|path| Finding {
            path: path.to_string(),
            lines: None,
            problem: format!("{path} drops the error"),
            why: "Saves look done".to_string(),
            blocks_anywhere: false,
            repeats_dropped: false,
        })
        .collect();
    second_round_finding(repo, &found, fix_diff).await
}

/// [`second_round`], with round 1 having found the Blockers `found`.
async fn second_round_finding(
    repo: &FakeRepo,
    found: &[Finding],
    fix_diff: bool,
) -> (RunId, ReviewRound) {
    let run_id = RunId::generate();
    let RunInsert::Inserted(_) = repo
        .insert_run(NewRun {
            id: run_id,
            flow: "code",
            request: "Add dark mode",
            user_id: &JULIA,
            task_id: None,
            project_id: None,
            repository: Some(&REPOSITORY),
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
        panic!("a run with no task is kept");
    };
    let entry = EntryId::try_from("review".to_string()).expect("entry");
    let start = |seq, round, head: &str, diff_base: &str, fix_diff| ReviewRoundStart {
        seq,
        round,
        reviewers: 1,
        head_sha: head.to_string(),
        diff_base: diff_base.to_string(),
        fix_diff,
    };
    repo.start_review_round(run_id, &start(3, 1, "aaa", "trunk", false))
        .await
        .expect("round 1");
    let first = StepKey { run_id, seq: 3 };
    repo.start_step(first, &entry, 1).await.expect("start");
    repo.record_findings(first, 3, found).await.expect("record");
    let classes: Vec<FindingClass> = (1..)
        .zip(found)
        .map(|(place, _)| FindingClass {
            seq: 3,
            place,
            kind: FindingKind::Blocker,
        })
        .collect();
    repo.classify_findings(run_id, 3, &classes)
        .await
        .expect("classify");
    repo.end_review_round(run_id, 3, ReviewRoundOutcome::Changes)
        .await
        .expect("end");

    let base = if fix_diff { "aaa" } else { "trunk" };
    let round = repo
        .start_review_round(run_id, &start(5, 2, "bbb", base, fix_diff))
        .await
        .expect("round 2");
    repo.start_step(StepKey { run_id, seq: 5 }, &entry, 2)
        .await
        .expect("start");
    (run_id, round)
}

/// The reviewer's Step 5 of `run_id`, in `round`.
fn reviewing<'a>(run_id: RunId, round: &'a ReviewRound) -> StepInput<'a> {
    StepInput {
        key: StepKey { run_id, seq: 5 },
        role: Some(&CORRECTNESS),
        owner: &JULIA,
        request: "Add dark mode",
        task_id: None,
        repository: Some(&REPOSITORY),
        earlier: &[],
        review: Some(round),
        reviewer: true,
        may_ask: false,
        questions_left: 0,
        resumes: None,
        created_in_person: false,
        control_file_patterns: None,
        definition_repo: None,
        owner_notes: &[],
        base_commit: None,
        routes: &[],
        max_visits: None,
        retry: None,
    }
}

fn steps(
    reviewer: &Reviewer,
    repo: &FakeRepo,
) -> BuiltInSteps<Reviewer, FakeGithub, FakeRepo, FakeWatches> {
    // A fix's work is on the Run's branch, at a commit no round reviewed.
    steps_at(reviewer, repo, &"f".repeat(40))
}

/// [`steps`], with the Run's branch at `head` on GitHub.
fn steps_at(
    reviewer: &Reviewer,
    repo: &FakeRepo,
    head: &str,
) -> BuiltInSteps<Reviewer, FakeGithub, FakeRepo, FakeWatches> {
    let github = FakeGithub::default();
    github.state().head = Some(head.to_string());
    BuiltInSteps::new(
        reviewer.clone(),
        PullRequestSteps::new(github, repo.clone(), FakeWatches::default()),
    )
}

fn review() -> StepName {
    StepName::try_from("review".to_string()).expect("valid")
}

/// A reviewer is told what its Role looks at, the commit it reviews, the
/// fix diff, and the Blockers of the round before, numbered, in a block
/// marked as reviewers' findings, to check wherever they are now.
#[tokio::test]
async fn a_reviewer_is_told_its_focus_the_fix_diff_and_the_blockers_to_check_again() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &["a.rs", "b.rs"], true).await;
    let reviewer = Reviewer::answering(("pass", PASSES), ("pass", PASSES));

    steps(&reviewer, &repo)
        .run(&review(), reviewing(run_id, &round))
        .await
        .expect("review");

    let prompt = reviewer.tasks.lock().expect("tasks")[0].prompt.clone();
    assert!(prompt.contains("Review commit `bbb`"), "{prompt}");
    assert!(prompt.contains("`git diff aaa...bbb`"), "{prompt}");
    assert!(
        prompt.contains("since review round 1 reviewed `aaa`"),
        "{prompt}"
    );
    assert!(
        prompt.contains("You review the change's correctness"),
        "{prompt}"
    );
    assert!(
        prompt.contains("## Blockers of the round before"),
        "{prompt}"
    );
    assert!(
        prompt.contains("1. `a.rs`: a.rs drops the error"),
        "{prompt}"
    );
    assert!(
        prompt.contains("2. `b.rs`: b.rs drops the error"),
        "{prompt}"
    );
    assert!(prompt.contains("<reviewer-findings>"), "{prompt}");
}

/// A first round reviews the whole change against the base, with nothing
/// carried over.
#[tokio::test]
async fn a_round_with_no_blockers_before_reviews_without_them() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &[], false).await;
    let reviewer = Reviewer::answering(("pass", PASSES), ("pass", PASSES));

    steps(&reviewer, &repo)
        .run(&review(), reviewing(run_id, &round))
        .await
        .expect("review");

    let prompt = reviewer.tasks.lock().expect("tasks")[0].prompt.clone();
    assert!(prompt.contains("`git diff origin/trunk...bbb`"), "{prompt}");
    assert!(!prompt.contains("Blockers of the round before"), "{prompt}");
}

/// A Blocker found again names its number in the round before: it is
/// recorded to block wherever it is, and the answer was read.
#[tokio::test]
async fn a_blocker_found_again_is_recorded_to_block_anywhere() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &["a.rs", "b.rs"], true).await;
    let again = "B is still there.\n```json\n{\"blockers\": [{\"file\": \"b.rs\", \
        \"problem\": \"Still drops the error\", \"why\": \"Saves look done\", \"previous\": 2}]}\n```";
    let reviewer = Reviewer::answering(("changes", again), ("pass", PASSES));

    let report = steps(&reviewer, &repo)
        .run(&review(), reviewing(run_id, &round))
        .await
        .expect("review");

    assert_eq!(report.outcome.as_str(), "changes");
    assert!(report.output.contains("still there from the round before"));
    let rounds = repo.review_rounds(run_id).await.expect("rounds");
    assert!(rounds[1].findings[0].blocks_anywhere);
    assert_eq!(rounds[1].answered, [5]);
    assert!(reviewer.turns.lock().expect("turns").is_empty());
}

/// An answer that does not read is asked for once more in the reviewer's
/// session, saying why; one that reads then is the reviewer's answer.
#[tokio::test]
async fn an_unreadable_answer_is_asked_for_again_once() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &[], true).await;
    let reviewer = Reviewer::answering(("pass", "Looks good to me."), ("pass", PASSES));

    let report = steps(&reviewer, &repo)
        .run(&review(), reviewing(run_id, &round))
        .await
        .expect("review");

    assert_eq!(report.outcome.as_str(), "pass");
    let turns = reviewer.turns.lock().expect("turns").clone();
    assert_eq!(turns.len(), 1);
    assert!(
        turns[0].prompt.contains("did not read"),
        "{}",
        turns[0].prompt
    );
    assert_eq!(turns[0].role, *CORRECTNESS);
    let rounds = repo.review_rounds(run_id).await.expect("rounds");
    assert!(rounds[1].findings.is_empty());
    assert_eq!(rounds[1].answered, [5]);
}

/// An answer that still does not read is one Blocker on the whole change:
/// the Step ends `changes`, never failed, and the round cannot pass on it.
#[tokio::test]
async fn an_answer_that_never_reads_is_one_blocker_on_the_whole_change() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &[], true).await;
    let reviewer = Reviewer::answering(("pass", "Fine."), ("pass", "Still fine."));

    let report = steps(&reviewer, &repo)
        .run(&review(), reviewing(run_id, &round))
        .await
        .expect("review");

    assert_eq!(report.outcome.as_str(), "changes");
    let rounds = repo.review_rounds(run_id).await.expect("rounds");
    let found = &rounds[1].findings;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].path, WHOLE_CHANGE);
    assert!(found[0].blocks_anywhere);
}

/// A retry of a Step whose re-ask was lost asks again in the same prompt,
/// so the session takes it as the one it already has.
#[tokio::test]
async fn a_retried_re_ask_asks_in_the_same_prompt() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &[], true).await;
    let reviewer = Reviewer::answering(("pass", "Looks good to me."), ("pass", PASSES));
    *reviewer.turns_failing.lock().expect("failing") = 1;
    let steps = steps(&reviewer, &repo);

    let lost = steps.run(&review(), reviewing(run_id, &round)).await;
    let retried = steps.run(&review(), reviewing(run_id, &round)).await;

    assert!(lost.is_err());
    assert_eq!(retried.expect("retried").outcome.as_str(), "pass");
    let turns = reviewer.turns.lock().expect("turns").clone();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].action_id, turns[1].action_id);
}

/// The tests reviewer is told how the factory ran each new test's break,
/// as data; another reviewer is not.
#[tokio::test]
async fn the_tests_reviewer_reads_the_break_records() {
    let repo = FakeRepo::default();
    let (run_id, round) = second_round(&repo, &[], false).await;
    let write = StepKey { run_id, seq: 2 };
    let entry = EntryId::try_from("write".to_string()).expect("entry");
    repo.start_step(write, &entry, 1).await.expect("start");
    let reported = crate::domain::breaks::read_line(
        "BREAK toggle_saves: src/toggle.rs \"true\" -> \"false\" run `cargo test toggle`",
    )
    .expect("a break");
    repo.keep_break(
        write,
        &reported.record(
            factory_records::domain::breaks::BreakStatus::NotReproduced,
            Some(&"a".repeat(40)),
            "1 passed",
        ),
    );
    let tests = Role::try_from("tests-reviewer".to_string()).expect("valid");
    let reviewer = Reviewer::answering(("pass", PASSES), ("pass", PASSES));
    let correctness = Reviewer::answering(("pass", PASSES), ("pass", PASSES));

    steps(&reviewer, &repo)
        .run(
            &review(),
            StepInput {
                role: Some(&tests),
                ..reviewing(run_id, &round)
            },
        )
        .await
        .expect("review");
    steps(&correctness, &repo)
        .run(&review(), reviewing(run_id, &round))
        .await
        .expect("review");

    let prompt = reviewer.tasks.lock().expect("tasks")[0].prompt.clone();
    assert!(prompt.contains("You review the change's tests"), "{prompt}");
    assert!(prompt.contains("## Break records"), "{prompt}");
    assert!(
        prompt.contains("1. not reproduced: it passed with the break too, so it does not catch it (step 2). Test `toggle_saves`."),
        "{prompt}"
    );
    assert!(prompt.contains("It said: 1 passed"), "{prompt}");
    let other = correctness.tasks.lock().expect("tasks")[0].prompt.clone();
    assert!(!other.contains("Break records"), "{other}");
}
