use chrono::Utc;
use factory_records::domain::models::{
    CheckResult, CheckRound, CheckRoundEnd, CheckRoundOutcome, EntryId, Outcome, RunId, Step,
    StepEnd,
};
use factory_records::domain::review_rounds::{
    DisputeRuling, FindingKind, FixStatus, Lines, ReviewFinding, ReviewRound, ReviewRoundEnd,
    ReviewRoundOutcome,
};
use uuid::Uuid;

use super::{input, outcome, steps};
use crate::domain::acceptance::plan_output;
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::models::{HeadChecks, StepInput};

const PLAN: &str = r#"Change the slug function so repeated dashes collapse.

```json
{"acceptance": [
  {"clause": "Repeated dashes collapse", "run": "cargo test slug"},
  {"clause": "The slug reads well", "none": "a person reads it"},
  {"clause": "The page loads", "run": "cargo test page"}
]}
```"#;

fn step(seq: u16, entry: &str, outcome: &str, output: String, checks: Vec<CheckRound>) -> Step {
    Step {
        seq,
        entry: EntryId::try_from(entry.to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        end: Some(StepEnd {
            outcome: Outcome::try_from(outcome.to_string()).expect("valid"),
            output,
            ended_at: Utc::now(),
        }),
        gate: None,
        role: None,
        agent: None,
        passed_over: Vec::new(),
        checks,
        terminal: Default::default(),
    }
}

fn check_round(
    checks: &[&str],
    outcome: CheckRoundOutcome,
    results: Vec<CheckResult>,
) -> CheckRound {
    CheckRound {
        round: 1,
        checks: checks.iter().map(|check| check.to_string()).collect(),
        started_at: Utc::now(),
        end: Some(CheckRoundEnd {
            outcome,
            report: String::new(),
            ended_at: Utc::now(),
        }),
        verified: true,
        results,
        checked_commit: None,
        work_output: String::new(),
        commands: Vec::new(),
        declared: String::new(),
        setup: None,
        check_action_id: Uuid::nil(),
        fix_action_id: None,
        quota_ran_out: false,
    }
}

fn result(name: &str, exit_code: i32) -> CheckResult {
    CheckResult {
        name: name.to_string(),
        exit_code,
        output: String::new(),
    }
}

/// The Steps of a Run that planned, wrote with its checks passing, and was
/// verified: the first acceptance check passed, the third failed.
fn earlier() -> Vec<Step> {
    vec![
        step(1, "plan", "approve", plan_output(PLAN), Vec::new()),
        step(
            2,
            "write",
            "done",
            "Wrote it.".to_string(),
            vec![check_round(
                &["test", "lint"],
                CheckRoundOutcome::Passed,
                Vec::new(),
            )],
        ),
        step(
            4,
            "accept",
            "pass",
            "Each clause holds.".to_string(),
            vec![check_round(
                &["acceptance-1", "acceptance-3"],
                CheckRoundOutcome::Failed,
                vec![result("acceptance-1", 0), result("acceptance-3", 1)],
            )],
        ),
    ]
}

fn finding(seq: u16, place: u16, path: &str, problem: &str, kind: FindingKind) -> ReviewFinding {
    ReviewFinding {
        seq,
        place,
        role: None,
        path: path.to_string(),
        lines: Some(Lines { start: 4, end: 6 }),
        problem: problem.to_string(),
        why: "It must.".to_string(),
        blocks_anywhere: false,
        repeats_dropped: false,
        fix_status: None,
        dispute: None,
        ruling: None,
        ruled_by: None,
        kind: Some(kind),
        follow_up_task_id: None,
        follow_up_key: None,
        follow_up_commented: false,
    }
}

fn round(
    seq: u16,
    fix_diff: bool,
    outcome: ReviewRoundOutcome,
    findings: Vec<ReviewFinding>,
) -> ReviewRound {
    ReviewRound {
        round: 0,
        visit: 1,
        seq,
        reviewers: 1,
        head_sha: Some(FakeGithub::HEAD.to_string()),
        diff_base: Some("main".to_string()),
        fix_diff,
        started_at: Utc::now(),
        failure: None,
        end: Some(ReviewRoundEnd {
            outcome,
            ended_at: Utc::now(),
        }),
        findings,
        answered: Vec::new(),
    }
}

/// A first round that found two Blockers, one fixed and one disputed and
/// dropped, and a follow-up filed as a task.
fn first_round() -> ReviewRound {
    let mut fixed = finding(
        3,
        1,
        "src/slug.rs",
        "Dashes are not collapsed, says @alice in #12",
        FindingKind::Blocker,
    );
    fixed.fix_status = Some(FixStatus::Fixed);
    let mut disputed = finding(
        3,
        2,
        "src/slug.rs",
        "The name is unclear",
        FindingKind::Blocker,
    );
    disputed.fix_status = Some(FixStatus::Disputed);
    disputed.ruling = Some(DisputeRuling::Dropped);
    let mut follow_up = finding(
        3,
        3,
        "src/page.rs",
        "The page has no test",
        FindingKind::FollowUp,
    );
    follow_up.follow_up_task_id = Some(Uuid::now_v7());
    round(
        3,
        false,
        ReviewRoundOutcome::Changes,
        vec![fixed, disputed, follow_up],
    )
}

fn keep_round(repo: &FakeRepo, run_id: RunId, round: ReviewRound) {
    repo.review_rounds
        .lock()
        .expect("rounds")
        .push((run_id, round));
}

/// The pull request opens with the request, then the report: the plan, the
/// Done-when checklist with each clause's result, the review rounds with
/// how each Blocker ended, the checks and the follow-ups; then the lines
/// that name the Run and its task. What agents wrote mentions no one.
#[tokio::test]
async fn publishing_writes_the_plan_checklist_and_review_rounds_into_the_description() {
    let github = FakeGithub::default();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    keep_round(&repo, run_id, first_round());
    keep_round(
        &repo,
        run_id,
        round(6, true, ReviewRoundOutcome::Pass, Vec::new()),
    );
    let earlier = earlier();
    let mut publishing: StepInput<'_> = input(run_id);
    publishing.earlier = &earlier;

    let report = steps(&github, &repo)
        .publish(publishing)
        .await
        .expect("publish");

    assert_eq!(outcome(&report), "published");
    let body = github.state().bodies[0].clone();
    assert!(body.starts_with("Collapse repeated dashes in slugs\n\nAnd add a test.\n\n"));
    for line in [
        "## Factory run",
        "> Change the slug function so repeated dashes collapse.",
        "- [x] Repeated dashes collapse: `cargo test slug` passed",
        "- [x] The slug reads well: checked by hand by the verifier",
        "- [ ] The page loads: `cargo test page` failed",
        "- Round 1, the whole change: 2 Blockers",
        "  - `src/slug.rs:4-6`: Dashes are not collapsed, says @\u{2060}alice in #\u{2060}12 (fixed)",
        "  - `src/slug.rs:4-6`: The name is unclear (disputed; the reviewer dropped it)",
        "- Round 2, the fixes: passed",
        "The repository's checks passed in round 1 of the `write` step: `test`, `lint`.",
        "- `src/page.rs:4-6`: The page has no test (filed as a task)",
    ] {
        assert!(body.contains(line), "{line:?} is not in:\n{body}");
    }
    // The acceptance block the plan keeps is not quoted.
    assert!(!body.contains("```json"));
    let report_end = body.find("<!-- /macro-factory-report -->").expect("marked");
    assert!(body[report_end..].contains("\n\nOpened by a Macro factory run ("));
    assert!(body[report_end..].contains("\n\nTask: MACRO-"));
    // It all fits: nothing was written after the pull request opened.
    assert!(github.state().description_edits.is_empty());
    assert!(github.state().comments.is_empty());
}

/// A request too long for the description is kept whole lines, and the
/// rest of it goes on in a comment, then the report; the lines that name
/// the Run and its task stay in the description.
#[tokio::test]
async fn a_request_too_long_for_the_description_goes_on_in_a_comment_by_whole_lines() {
    let github = FakeGithub::default();
    // Characters, not bytes, count: each line holds a two-byte one.
    let request: String = (0..2_500)
        .map(|line| format!("Line {line}: collapse the dashes é.\n"))
        .collect();
    let mut long = input(RunId::generate());
    long.request = &request;

    steps(&github, &FakeRepo::default())
        .publish(long)
        .await
        .expect("publish");

    let body = github.state().description.clone().expect("opened");
    let comments = github.state().comments.clone();
    assert!(body.chars().count() <= 65_536);
    assert!(body.contains("\n\nTask: MACRO-"));
    assert_eq!(comments.len(), 1);
    let comment = &comments[0].body;
    assert!(comment.chars().count() <= 65_536);
    assert!(comment.starts_with("<!-- macro-factory-report-part 1 -->\n"));
    // The report follows, where room is left for it.
    assert_eq!(
        format!("{body}{comment}").matches("## Factory run").count(),
        1
    );

    let (kept, _) = body
        .split_once("\n(The rest of the request is in the comments below.)\n")
        .expect("cut");
    let (_, rest) = comment
        .split_once("The rest of the request:\n\n")
        .expect("goes on");
    let rest = rest
        .split("\n\nThe rest of the factory run's report:")
        .next()
        .unwrap_or_default();
    let rest = rest.strip_suffix("\n\n").unwrap_or(rest);
    assert!(kept.ends_with('\n'), "the request is cut mid-line");
    assert_eq!(format!("{kept}{rest}"), request);
}

/// A later visit replaces the report's section in place: what a person
/// wrote around it stays, there is one report, and a visit with nothing
/// new writes nothing.
#[tokio::test]
async fn a_later_visit_replaces_the_report_in_place_keeping_what_a_person_wrote() {
    let github = FakeGithub::default();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    keep_round(&repo, run_id, first_round());

    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish");
    {
        let mut state = github.state();
        let opened = state.description.clone().expect("opened");
        state.description = Some(format!(
            "A note from a person.\n\n{opened}\n\nReviewed by hand."
        ));
    }
    keep_round(
        &repo,
        run_id,
        round(6, true, ReviewRoundOutcome::Pass, Vec::new()),
    );
    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish again");
    steps(&github, &repo)
        .publish(input(run_id))
        .await
        .expect("publish with nothing new");

    let state = github.state();
    let body = state.description.clone().expect("opened");
    assert_eq!(state.opened, 1);
    assert_eq!(state.description_edits.len(), 1);
    assert!(body.starts_with("A note from a person.\n\nCollapse repeated dashes in slugs"));
    assert!(body.ends_with("\n\nReviewed by hand."));
    assert_eq!(body.matches("## Factory run").count(), 1);
    assert!(body.contains("- Round 1, the whole change: 2 Blockers"));
    assert!(body.contains("- Round 2, the fixes: passed"));
}

/// When what a person wrote leaves the report no room, the rest of it goes
/// on in a comment; once it fits again, the comment says it is no longer
/// needed.
#[tokio::test]
async fn a_report_without_room_goes_on_in_a_comment_until_it_fits_again() {
    let github = FakeGithub::default();
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    let plan: String = (0..60)
        .map(|line| format!("Step {line} of the plan, which changes one thing.\n"))
        .collect();
    let earlier = vec![step(1, "plan", "approve", plan, Vec::new())];
    let mut publishing: StepInput<'_> = input(run_id);
    publishing.earlier = &earlier;
    let note = "A long note from a person.\n".repeat(2_400);

    steps(&github, &repo)
        .publish(publishing)
        .await
        .expect("publish");
    {
        let mut state = github.state();
        let opened = state.description.clone().expect("opened");
        state.description = Some(format!("{note}{opened}"));
    }
    steps(&github, &repo)
        .publish(publishing)
        .await
        .expect("publish without room");

    let crowded = github.state().description.clone().expect("opened");
    let comments = github.state().comments.clone();
    assert!(crowded.chars().count() <= 65_536);
    assert!(crowded.contains("(The rest of this report is in the comments below.)"));
    assert_eq!(comments.len(), 1);
    assert!(
        comments[0]
            .body
            .contains("The rest of the factory run's report:")
    );
    assert!(comments[0].body.contains("> Step 59 of the plan"));

    {
        let mut state = github.state();
        state.description = Some(crowded.replacen(&note, "", 1));
    }
    steps(&github, &repo)
        .publish(publishing)
        .await
        .expect("publish with room");

    let state = github.state();
    let body = state.description.clone().expect("opened");
    assert!(body.contains("> Step 59 of the plan"));
    assert!(!body.contains("(The rest of this report is in the comments below.)"));
    assert_eq!(state.comments.len(), 1);
    assert_eq!(
        state.comments[0].body,
        "<!-- macro-factory-report-part 1 -->\n\
         (Nothing more: the whole report fits in the description now.)"
    );
}

/// `ci` writes the report once as it starts, so a Run that comes back to
/// it without `publish`, as from `update`, shows its latest rounds.
#[tokio::test(start_paused = true)]
async fn ci_writes_the_report_before_it_waits() {
    let github = FakeGithub::with_open_pull_request(ci(Some("success")));
    let repo = FakeRepo::default();
    let run_id = RunId::generate();
    keep_round(&repo, run_id, first_round());

    let report = steps(&github, &repo).ci(input(run_id)).await.expect("ci");

    assert_eq!(outcome(&report), "green");
    let state = github.state();
    assert_eq!(state.description_edits.len(), 1);
    assert!(
        state.description_edits[0].starts_with("<!-- macro-factory-report -->\n## Factory run")
    );
    assert!(state.description_edits[0].contains("- Round 1, the whole change: 2 Blockers"));
}

/// A report GitHub refuses to write does not hold the pull request: it is
/// still published.
#[tokio::test]
async fn a_report_that_cannot_be_written_still_publishes() {
    let github = FakeGithub::with_open_pull_request(HeadChecks::default());
    github.state().text_edits_fail = true;

    let report = steps(&github, &FakeRepo::default())
        .publish(input(RunId::generate()))
        .await
        .expect("publish");

    assert_eq!(outcome(&report), "published");
    assert!(github.state().description_edits.is_empty());
}
