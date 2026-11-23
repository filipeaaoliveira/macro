//! The breaks a write or fix Step's agent reports, run by the round of
//! checks that passed, on the commit it checked.

use factory_records::domain::breaks::{BreakRecord, BreakStatus};

use super::verified::{ran, run_in_session, verifying};
use super::*;

/// One break of the test `toggle_saves`.
const BREAK: &str = "BREAK toggle_saves: apps/web/src/toggle.ts \"saved = true\" -> \"saved = false\" run `bun test toggle`";

/// What the break's run says when the test passed without it and failed
/// with it.
const VERIFIED: &str =
    "macro-break NONCE: unbroken 0\nmacro-break NONCE: broken 1\nmacro-break NONCE: restored\n";

/// An agent whose session runs commands, `runs` in turn, and whose work
/// hands back `work`.
fn working(work: &str, runs: Vec<Ran>, later: Vec<Scripted>) -> ScriptedAgent {
    ScriptedAgent {
        work_text: work.to_string(),
        ..verifying(runs, later)
    }
}

fn breaks(repo: &FakeRepo, input: StepInput<'_>) -> Vec<BreakRecord> {
    repo.step_breaks(input.key)
}

/// The commands the factory ran that were breaks.
fn break_commands(agent: &ScriptedAgent) -> Vec<AgentCommand> {
    agent
        .commands
        .lock()
        .expect("commands")
        .iter()
        .filter(|command| command.command.command.contains("macro-break "))
        .cloned()
        .collect()
}

/// Once the factory's run of the checks passed, the reported break runs in
/// the round's worktree on the commit they checked, and its record says
/// the test failed with it.
#[tokio::test]
async fn a_break_whose_test_fails_is_recorded_verified_on_the_checked_commit() {
    let agent = working(
        &format!("Wrote the toggle.\n{BREAK}\n"),
        vec![
            ran(0, "12 passed"),
            Ok(SessionCommandOutput {
                exit_code: 0,
                stdout: VERIFIED.to_string(),
                stderr: "1 failed: toggle_saves".to_string(),
            }),
        ],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    let records = breaks(&repo, input);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].test, "toggle_saves");
    assert_eq!(records[0].status, BreakStatus::Verified);
    assert_eq!(records[0].checked_commit.as_deref(), Some(HEAD));
    assert_eq!(records[0].output, "1 failed: toggle_saves");
    assert_eq!(records[0].command, "bun test toggle");
    let sent = break_commands(&agent);
    assert_eq!(sent.len(), 1);
    let rounds = rounds(&repo, input).await;
    let round = rounds[0].check_action_id.to_string();
    // In the round's worktree, under its lock.
    let shell = &sent[0].command.command;
    assert!(shell.contains(&format!("worktree=\"$base/{round}\"")));
    assert!(
        shell.contains("timeout --kill-after=10 300 bash --noprofile --norc -c 'bun test toggle'")
    );
    assert!(shell.contains("BREAK_OLD='saved = true' BREAK_NEW='saved = false'"));
    // Two runs of five minutes, the lock's wait, two kills' ten seconds and
    // a minute for the rest.
    assert_eq!(sent[0].command.timeout_secs, 600 + 60 + 20 + 60);
    assert_eq!(sent[0].role.as_str(), "writer");
    // The break ran before the round ended and its worktree went.
    let machine = agent.machine.lock().expect("machine").clone();
    assert_eq!(machine.len(), 4);
    assert_eq!(machine[3], "remove");
}

/// A break the test passes with is recorded as not reproduced: the test
/// does not catch it.
#[tokio::test]
async fn a_break_whose_test_passes_is_recorded_not_reproduced() {
    let agent = working(
        BREAK,
        vec![
            ran(0, "12 passed"),
            ran(
                0,
                "macro-break NONCE: unbroken 0\nmacro-break NONCE: broken 0\nmacro-break NONCE: restored\n",
            ),
        ],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    let records = breaks(&repo, input);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, BreakStatus::NotReproduced);
}

/// A run that ended before it said how the test went is interrupted: the
/// attempt fails for a retry, with the round still open, and the retry
/// runs the break again; its record then says how it went.
#[tokio::test]
async fn an_interrupted_break_runs_again_on_the_retry() {
    let agent = working(
        BREAK,
        vec![
            ran(0, "12 passed"),
            ran(0, "macro-break NONCE: unbroken 0\n"),
        ],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let first = run_in_session(&agent, &github, &repo, input).await;

    assert!(first.is_err());
    let records = breaks(&repo, input);
    assert_eq!(records[0].status, BreakStatus::Interrupted);
    assert!(
        records[0]
            .output
            .starts_with("The break's run ended before it said how the test went (exit status 0)")
    );
    assert!(rounds(&repo, input).await[0].end.is_none());

    let retry = working(BREAK, vec![ran(0, VERIFIED)], Vec::new());
    let report = run_in_session(&retry, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    let records = breaks(&repo, input);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, BreakStatus::Verified);
    assert_eq!(break_commands(&retry).len(), 1);
}

/// A break interrupted on its retry too keeps that record, and the round
/// ends: it is not retried for ever.
#[tokio::test]
async fn a_break_interrupted_twice_stays_interrupted() {
    let agent = working(BREAK, vec![ran(0, "12 passed"), ran(137, "")], Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let earlier = crate::domain::breaks::read_line(BREAK).expect("a break");
    repo.keep_break(
        input.key,
        &earlier.record(BreakStatus::Interrupted, Some(HEAD), ""),
    );

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    let records = breaks(&repo, input);
    assert_eq!(records[0].status, BreakStatus::Interrupted);
    assert!(records[0].output.contains("(exit status 137)"));
}

/// A break's test runs after the repository's setup `env`, exactly as the
/// round's checks do, so a test that needs it passes without the break.
#[tokio::test]
async fn a_break_runs_with_the_checks_environment() {
    let agent = working(
        BREAK,
        vec![ran(0, "12 passed"), ran(0, VERIFIED)],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    github.state().files.insert(
        (
            "main".to_string(),
            crate::domain::setup::SETUP_FILE.to_string(),
        ),
        r#"{"env": {"DATABASE_URL": "postgres://db"}}"#.to_string(),
    );
    let repo = FakeRepo::default();
    let input = input();

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    let sent = agent.commands.lock().expect("commands").clone();
    let check = &sent[0].command.command;
    let shell = &break_commands(&agent)[0].command.command;
    assert!(check.contains("export DATABASE_URL="), "{check}");
    assert!(
        shell.contains("bash --noprofile --norc -c 'export DATABASE_URL="),
        "{shell}"
    );
    assert_eq!(breaks(&repo, input)[0].status, BreakStatus::Verified);
}

/// The markers may come back in either stream, as a runtime that merges
/// them returns them, and the test's output kept is both streams' without
/// the markers.
#[tokio::test]
async fn markers_read_from_a_merged_stream() {
    let agent = working(
        BREAK,
        vec![
            ran(0, "12 passed"),
            Ok(SessionCommandOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: format!(
                    "{}\n--- the test with the break exited with status 1\n{VERIFIED}",
                    "x".repeat(20_000)
                ),
            }),
        ],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    let records = breaks(&repo, input);
    assert_eq!(records[0].status, BreakStatus::Verified);
    assert!(records[0].output.ends_with("exited with status 1"));
    assert!(!records[0].output.contains("macro-break"));
}

/// A work that reports no break runs none and records none: the reviewer
/// then sees that its new tests have none.
#[tokio::test]
async fn a_missing_break_runs_nothing_and_records_nothing() {
    let agent = working(
        "Wrote the toggle and its test.",
        vec![ran(0, "12 passed")],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert!(break_commands(&agent).is_empty());
    assert!(breaks(&repo, input).is_empty());
}

/// A round that failed runs no break: the fix changes the code. The round
/// that passes runs the breaks of the work and its fixes, a fix's line for
/// a test replacing the work's.
#[tokio::test]
async fn only_the_round_that_passes_runs_the_breaks_its_fix_replaced() {
    let agent = working(
        &format!(
            "{BREAK}\nBREAK toggle_keeps: apps/web/src/toggle.ts \"keep\" -> \"drop\" run `bun test keeps`"
        ),
        vec![
            ran(1, "toggle_saves failed"),
            ran(0, "12 passed"),
            ran(0, VERIFIED),
            ran(0, VERIFIED),
        ],
        vec![Ok((
            "done",
            "Fixed the toggle.\nBREAK toggle_saves: apps/web/src/toggle.ts \"saved\" -> \"lost\" run `bun test toggle`",
        ))],
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    let sent = break_commands(&agent);
    assert_eq!(sent.len(), 2);
    assert!(
        sent[0]
            .command
            .command
            .contains("BREAK_OLD='saved' BREAK_NEW='lost'")
    );
    assert!(sent[1].command.command.contains("BREAK_OLD='keep'"));
    let machine = agent.machine.lock().expect("machine").clone();
    // Round 1: set up, the check, remove; round 2: set up, the check, the
    // two breaks, remove.
    assert_eq!(machine.len(), 8, "{machine:?}");
    let records = breaks(&repo, input);
    let tests: Vec<(&str, BreakStatus)> = records
        .iter()
        .map(|record| (record.test.as_str(), record.status))
        .collect();
    assert_eq!(
        tests,
        [
            ("toggle_saves", BreakStatus::Verified),
            ("toggle_keeps", BreakStatus::Verified)
        ]
    );
    assert_eq!(records[0].old_text, "saved");
}

/// A retry finds the breaks an earlier attempt recorded, and runs only
/// those still to run.
#[tokio::test]
async fn a_retry_runs_only_the_breaks_still_to_run() {
    let agent = working(
        &format!(
            "{BREAK}\nBREAK toggle_keeps: apps/web/src/toggle.ts \"keep\" -> \"drop\" run `bun test keeps`"
        ),
        vec![ran(0, "12 passed"), ran(0, VERIFIED)],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let earlier = crate::domain::breaks::read_line(BREAK).expect("a break");
    repo.keep_break(
        input.key,
        &earlier.record(BreakStatus::Verified, Some(HEAD), ""),
    );

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    let sent = break_commands(&agent);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].command.command.contains("BREAK_OLD='keep'"));
    assert_eq!(breaks(&repo, input).len(), 2);
}

/// An unreadable line is recorded unreadable and runs nothing, and does
/// not count among the breaks a Step runs; runnable breaks past that most
/// are recorded dropped.
#[tokio::test]
async fn unreadable_and_surplus_breaks_run_nothing() {
    let most = crate::domain::breaks::MAX_BREAKS;
    let mut work = String::from("BREAK toggle_saves: apps/web/src/toggle.ts\n");
    for test in 1..=most + 1 {
        work.push_str(&format!(
            "BREAK test_{test}: apps/web/src/toggle.ts \"a\" -> \"b\" run `bun test {test}`\n"
        ));
    }
    let mut runs = vec![ran(0, "12 passed")];
    runs.extend((1..=most).map(|_| ran(0, VERIFIED)));
    let agent = working(&work, runs, Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(break_commands(&agent).len(), most);
    let records = breaks(&repo, input);
    assert_eq!(records.len(), most + 2);
    assert_eq!(records[0].status, BreakStatus::Unreadable);
    assert_eq!(
        records[0].test,
        "BREAK toggle_saves: apps/web/src/toggle.ts"
    );
    assert_eq!(records[most].status, BreakStatus::Verified);
    let last = records.last().expect("records");
    assert_eq!(last.test, format!("test_{}", most + 1));
    assert_eq!(last.status, BreakStatus::Dropped);
}

/// A Step keeps records of at most fifty break lines.
#[tokio::test]
async fn a_step_keeps_at_most_fifty_records() {
    let mut work = String::new();
    for test in 1..=60 {
        work.push_str(&format!("BREAK test_{test}: unreadable\n"));
    }
    let agent = working(&work, vec![ran(0, "12 passed")], Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(
        breaks(&repo, input).len(),
        crate::domain::breaks::MAX_REPORTED
    );
}

/// Where the agent's session runs no command, the agent runs the checks,
/// and the breaks are recorded unchecked: its word only.
#[tokio::test]
async fn breaks_the_factory_cannot_run_are_recorded_unchecked() {
    let agent = ScriptedAgent {
        work_text: BREAK.to_string(),
        ..ScriptedAgent::new("done", vec![Ok(("passed", "web: exit 0"))])
    };
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");

    let records = breaks(&repo, input);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, BreakStatus::Unchecked);
    assert_eq!(records[0].checked_commit, None);
}

/// A repository that declares no checks runs none, and so no break: the
/// breaks are recorded unchecked.
#[tokio::test]
async fn breaks_of_a_repository_without_checks_are_recorded_unchecked() {
    let agent = ScriptedAgent {
        work_text: BREAK.to_string(),
        ..ScriptedAgent::new("done", Vec::new())
    };
    let github = github(None, Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    let records = breaks(&repo, input);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, BreakStatus::Unchecked);
}

/// A break that would not end before the Step's time for rounds is
/// recorded dropped, and the round still ends.
#[tokio::test]
async fn a_break_with_no_time_left_is_dropped() {
    let agent = working(BREAK, vec![ran(0, "12 passed")], Vec::new());
    let github = github(
        Some(
            r#"{"rounds": 1, "checks": [{"name": "web", "run": "bun run check", "paths": ["apps/web"], "timeout": 60}]}"#,
        ),
        Some(&["apps/web/src/main.ts"]),
    );
    let repo = FakeRepo::default();
    let input = input();
    // 100 of the rounds' 105 minutes are gone: the check's minute fits,
    // the break's two runs of five minutes do not.
    super::verified::started_ago(&repo, input.key, 100).await;

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    assert!(break_commands(&agent).is_empty());
    let records = breaks(&repo, input);
    assert_eq!(records[0].status, BreakStatus::Dropped);
    assert_eq!(
        records[0].output,
        "The step's time ran out before this break ran."
    );
}
