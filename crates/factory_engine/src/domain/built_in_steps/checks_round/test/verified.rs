//! Rounds whose checks the engine runs itself, in the agent's checkout,
//! reading each one's exit code.

use factory_records::domain::models::MAX_CHECK_RESULT_OUTPUT_BYTES;

use super::*;

/// Both checks of [`CHECKS`] run: the branch changed both their paths.
const BOTH: &[&str] = &["apps/web/src/main.ts", "crates/app/src/lib.rs"];

fn ran(exit_code: i32, stdout: &str) -> Ran {
    Ok(SessionCommandOutput {
        exit_code,
        stdout: stdout.to_string(),
        stderr: String::new(),
    })
}

/// An agent whose session runs commands, each ending as `runs` say in
/// turn, and whose later prompts end as `later` say.
fn verifying(runs: Vec<Ran>, later: Vec<Scripted>) -> ScriptedAgent {
    ScriptedAgent {
        checkout: Some(Arc::new(Mutex::new(runs.into()))),
        ..ScriptedAgent::new("done", later)
    }
}

fn commands(agent: &ScriptedAgent) -> Vec<String> {
    agent
        .commands
        .lock()
        .expect("commands")
        .iter()
        .map(|command| super::run_of(&command.command.command))
        .collect()
}

/// [`run`], for a Step whose agent works in a session it claimed, as a
/// real agent's does.
async fn run_in_session(
    agent: &ScriptedAgent,
    github: &FakeGithub,
    repo: &FakeRepo,
    input: StepInput<'_>,
) -> Result<StepReport, super::super::BuiltInStepError> {
    let entry = EntryId::try_from("write".to_string()).expect("valid");
    repo.start_step(input.key, &entry, 1).await.expect("start");
    if repo
        .claimed_session(input.key)
        .await
        .expect("read")
        .is_none()
    {
        repo.claim_session(
            input.key,
            StepSession {
                session_id: Uuid::now_v7(),
                action_id: Uuid::now_v7(),
                agent: Some(AgentName::try_from("primary".to_string()).expect("valid")),
                bot: Some(Uuid::now_v7()),
            },
            &Role::try_from("writer".to_string()).expect("valid"),
            &[Outcome::done()],
        )
        .await
        .expect("claim");
    }
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let step = StepName::try_from("write".to_string()).expect("valid");
    steps.run(&step, input).await
}

/// Where the agent's session runs commands, the engine runs the checks in
/// its checkout and their exit codes decide the round: the agent is never
/// asked to run them.
#[tokio::test]
async fn checks_the_factory_runs_decide_the_round() {
    let agent = verifying(vec![ran(0, "12 passed")], Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    assert!(report.output.starts_with(
        "The repository's checks passed in round 1, run by the factory on the pushed commit: web."
    ));
    assert!(agent.turns().is_empty());
    let sent = agent.commands.lock().expect("commands").clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].command.cwd, ".");
    assert_eq!(super::run_of(&sent[0].command.command), "bun run check");
    // Its ten minutes, and the round lock's wait on top.
    assert_eq!(sent[0].command.timeout_secs, 600 + 80);
    assert!(
        sent[0]
            .command
            .command
            .contains("timeout --kill-after=10 600 bash")
    );
    assert_eq!(sent[0].role.as_str(), "writer");
    let rounds = rounds(&repo, input).await;
    assert!(rounds[0].verified);
    assert_eq!(rounds[0].checked_commit.as_deref(), Some(HEAD));
    assert_eq!(rounds[0].results.len(), 1);
    assert_eq!(
        (
            rounds[0].results[0].name.as_str(),
            rounds[0].results[0].exit_code
        ),
        ("web", 0)
    );
    assert_eq!(
        rounds[0].end.as_ref().expect("ended").outcome,
        CheckRoundOutcome::Passed
    );
}

/// A failed round goes back to the agent with the checks that failed and
/// the end of their output, not the ones that passed; the next round runs
/// the checks again on its fix.
#[tokio::test]
async fn the_fix_prompt_holds_only_the_failures() {
    let long = format!("{}test toggle_saves failed", "x".repeat(10_000));
    let agent = verifying(
        vec![
            ran(0, "12 passed"),
            ran(101, &long),
            ran(0, "12 passed"),
            ran(0, "ok"),
        ],
        vec![Ok(("done", "Fixed the toggle's saving"))],
    );
    let github = github(Some(CHECKS), Some(BOTH));
    let repo = FakeRepo::default();
    let input = input();

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    let turns = agent.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].outcomes, [Outcome::done()]);
    assert!(
        turns[0]
            .prompt
            .contains("`rust`: `cargo test -p app` exited with status 101")
    );
    assert!(turns[0].prompt.contains("test toggle_saves failed"));
    assert!(!turns[0].prompt.contains("`web`"));
    assert!(!turns[0].prompt.contains("12 passed"));
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 2);
    assert!(rounds.iter().all(|round| round.verified));
    let rust = &rounds[0].results[1];
    assert_eq!((rust.name.as_str(), rust.exit_code), ("rust", 101));
    assert!(rust.output.len() <= 4096);
    assert!(rust.output.ends_with("test toggle_saves failed"));
    assert_eq!(rounds[1].results.len(), 2);
    assert_eq!(
        commands(&agent),
        [
            "bun run check",
            "cargo test -p app",
            "bun run check",
            "cargo test -p app"
        ]
    );
}

/// What the agent says never passes a check that failed: a fix it hands
/// back as working is checked again, and a round that failed stays failed
/// whatever the agent handed back in its session.
#[tokio::test]
async fn an_agent_s_word_never_overrides_a_real_failure() {
    let agent = verifying(
        vec![ran(1, "error: 3 failed"), ran(1, "error: 3 failed")],
        vec![Ok(("done", "All the checks pass now."))],
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, outcome("red"));
    assert!(report.output.starts_with(
        "The repository's checks still fail in round 2, run by the factory on the pushed commit, their last: web."
    ));
    assert!(report.output.contains("error: 3 failed"));
    let ends: Vec<CheckRoundOutcome> = rounds(&repo, input)
        .await
        .iter()
        .map(|round| round.end.as_ref().expect("ended").outcome)
        .collect();
    assert_eq!(ends, [CheckRoundOutcome::Failed, CheckRoundOutcome::Failed]);
}

/// A verified round still open when the Step's time runs out ends with
/// what the engine read, even when the agent handed `passed` back in its
/// session for it.
#[tokio::test]
async fn a_passed_hand_back_never_ends_a_verified_round() {
    let github = github(Some(CHECKS), Some(BOTH));
    let repo = FakeRepo::default();
    let input = input();
    let first = verifying(vec![ran(1, "web: 2 failed"), Err(FAILED)], Vec::new());
    let failed = run_in_session(&first, &github, &repo, input).await;
    let open = rounds(&repo, input).await.remove(0);
    let session = repo
        .claimed_session(input.key)
        .await
        .expect("read")
        .expect("claimed");
    repo.begin_turn(
        input.key,
        session.session_id,
        open.check_action_id,
        2,
        &[outcome("passed"), outcome("failed")],
    )
    .await
    .expect("turn");
    repo.hand_back(
        session.session_id,
        &HandBack {
            outcome: outcome("passed"),
            output: "Every check passes.".to_string(),
        },
    )
    .await
    .expect("hand back");
    age(&repo, input.key);
    let retry = verifying(Vec::new(), Vec::new());

    let report = run_in_session(&retry, &github, &repo, input)
        .await
        .expect("write");

    assert!(failed.is_err());
    assert_eq!(report.outcome, outcome("red"));
    assert!(retry.turns().is_empty());
    let end = rounds(&repo, input).await[0].end.clone().expect("ended");
    assert_eq!(end.outcome, CheckRoundOutcome::Failed);
    assert!(end.report.contains("web: 2 failed"));
    assert!(
        end.report
            .contains("The step's time ran out before these ran: rust.")
    );
    assert!(!end.report.contains("Every check passes."));
}

/// A retry goes on with the checks its round has not run yet: those that
/// ran keep the run they had.
#[tokio::test]
async fn a_retry_runs_only_the_checks_still_to_run() {
    let github = github(Some(CHECKS), Some(BOTH));
    let repo = FakeRepo::default();
    let input = input();
    let first = verifying(vec![ran(0, "12 passed"), Err(FAILED)], Vec::new());
    let failed = run_in_session(&first, &github, &repo, input).await;
    let retry = verifying(vec![ran(0, "ok")], Vec::new());

    let report = run_in_session(&retry, &github, &repo, input)
        .await
        .expect("write");

    assert!(failed.is_err());
    assert_eq!(report.outcome, Outcome::done());
    assert_eq!(commands(&retry), ["cargo test -p app"]);
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].results.len(), 2);
}

/// Where the session's runtime runs no command, the agent runs the checks
/// and reports them, as it always did.
#[tokio::test]
async fn a_runtime_that_runs_no_command_leaves_the_checks_to_the_agent() {
    let agent = ScriptedAgent::new("done", vec![Ok(("passed", "web: exit 0"))]);
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert!(
        report
            .output
            .starts_with("The repository's checks passed in round 1: web.")
    );
    assert_eq!(agent.turns().len(), 1);
    let rounds = rounds(&repo, input).await;
    assert!(!rounds[0].verified);
    assert!(rounds[0].results.is_empty());
}

/// A round whose checks an earlier attempt already asked the agent to run
/// stays the agent's, though its session runs commands now: the engine
/// does not run them under the agent.
#[tokio::test]
async fn a_round_the_agent_was_asked_to_run_stays_the_agents() {
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let first = ScriptedAgent::new("done", vec![Err(FAILED)]);
    let failed = run_in_session(&first, &github, &repo, input).await;
    let open = rounds(&repo, input).await.remove(0);
    let session = repo
        .claimed_session(input.key)
        .await
        .expect("read")
        .expect("claimed");
    // The real agent records the prompt before it sends it.
    repo.begin_turn(
        input.key,
        session.session_id,
        open.check_action_id,
        2,
        &[outcome("passed"), outcome("failed")],
    )
    .await
    .expect("turn");
    let retry = verifying(Vec::new(), vec![Ok(("passed", "web: exit 0"))]);

    let report = run_in_session(&retry, &github, &repo, input)
        .await
        .expect("write");

    assert!(failed.is_err());
    assert_eq!(report.outcome, Outcome::done());
    assert!(commands(&retry).is_empty());
    assert_eq!(retry.turns()[0].action_id, open.check_action_id);
    assert!(!rounds(&repo, input).await[0].verified);
}

/// Make the Step `key` as old as `minutes`: its rounds have 105 minus that
/// left.
pub(super) async fn started_ago(repo: &FakeRepo, key: StepKey, minutes: i64) {
    let entry = EntryId::try_from("write".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    let mut steps = repo.steps.lock().expect("steps");
    for (run, step) in steps.iter_mut() {
        if *run == key.run_id && step.seq == key.seq {
            step.started_at = chrono::Utc::now() - chrono::TimeDelta::minutes(minutes);
        }
    }
}

/// A check starts only with its whole timeout left in the Step's rounds:
/// short of it, the round and the Step end at once, red, saying what did
/// not run, and nothing runs that would be cut short.
#[tokio::test]
async fn a_round_short_of_a_check_s_timeout_ends_the_step_red() {
    let agent = verifying(vec![ran(0, "never run")], Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    // Five minutes of the Step's rounds are left, short of the check's ten.
    started_ago(&repo, input.key, 100).await;

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, outcome("red"));
    assert!(commands(&agent).is_empty());
    let end = rounds(&repo, input).await[0].end.clone().expect("ended");
    assert_eq!(end.outcome, CheckRoundOutcome::Failed);
    assert!(
        end.report
            .contains("The step's time ran out before these ran: web.")
    );
}

/// A check that declares a shorter timeout runs with less time left, under
/// that timeout.
#[tokio::test]
async fn a_short_check_runs_with_less_time_left() {
    let agent = verifying(vec![ran(0, "ok")], Vec::new());
    let github = github(
        Some(r#"{"checks":[{"name":"web","run":"bun run check","timeout":300}]}"#),
        None,
    );
    let repo = FakeRepo::default();
    let input = input();
    started_ago(&repo, input.key, 90).await;

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    let sent = agent.commands.lock().expect("commands").clone();
    assert_eq!(sent[0].command.timeout_secs, 300 + 80);
}

/// The worktree is set up before the round's checks, on the machine, and
/// removed after them.
#[tokio::test]
async fn the_worktree_is_set_up_before_the_checks_and_removed_after() {
    let agent = verifying(vec![ran(0, "12 passed"), ran(0, "ok")], Vec::new());
    let github = github(Some(CHECKS), Some(BOTH));
    let repo = FakeRepo::default();

    run_in_session(&agent, &github, &repo, input())
        .await
        .expect("write");

    assert_eq!(
        *agent.machine.lock().expect("machine"),
        [
            "set up",
            "check bun run check",
            "check cargo test -p app",
            "remove"
        ]
    );
}

/// A worktree that cannot be set up fails the attempt for a retry: no
/// check runs, and nothing is recorded as a failure.
#[tokio::test]
async fn a_worktree_that_cannot_be_set_up_is_left_to_a_retry() {
    let agent = ScriptedAgent {
        set_up: Some(ran(128, "fatal: not a git repository")),
        ..verifying(Vec::new(), Vec::new())
    };
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let failed = run_in_session(&agent, &github, &repo, input).await;

    assert!(matches!(
        failed,
        Err(super::super::super::BuiltInStepError::Agent {
            retryable: true,
            ..
        })
    ));
    assert!(commands(&agent).is_empty());
    let rounds = rounds(&repo, input).await;
    assert!(rounds[0].results.is_empty());
    assert!(rounds[0].end.is_none());
}

/// A branch GitHub does not have fails the attempt for a retry: nothing
/// runs on the machine.
#[tokio::test]
async fn a_branch_missing_on_github_is_left_to_a_retry() {
    let agent = verifying(Vec::new(), Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    github.state().head = None;
    let repo = FakeRepo::default();
    let input = input();

    let failed = run_in_session(&agent, &github, &repo, input).await;

    assert!(matches!(
        failed,
        Err(super::super::super::BuiltInStepError::Agent {
            retryable: true,
            ..
        })
    ));
    assert!(agent.machine.lock().expect("machine").is_empty());
    assert!(rounds(&repo, input).await[0].end.is_none());
}

/// A run that found another holding the round's lock is left to a retry,
/// and recorded as nothing: the retry runs the check.
#[tokio::test]
async fn a_run_that_found_the_lock_held_is_retried() {
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let first = verifying(
        vec![Ok(SessionCommandOutput {
            exit_code: 75,
            stdout: String::new(),
            stderr: "macro: another run holds this lock\n".to_string(),
        })],
        Vec::new(),
    );
    let failed = run_in_session(&first, &github, &repo, input).await;
    let recorded = rounds(&repo, input).await[0].results.len();
    let retry = verifying(vec![ran(0, "12 passed")], Vec::new());

    let report = run_in_session(&retry, &github, &repo, input)
        .await
        .expect("write");

    assert!(matches!(
        failed,
        Err(super::super::super::BuiltInStepError::Agent {
            retryable: true,
            ..
        })
    ));
    assert_eq!(recorded, 0);
    assert_eq!(report.outcome, Outcome::done());
    assert_eq!(commands(&retry), ["bun run check"]);
}

/// What a round keeps of a check's output fits its record once escaped as
/// JSON, keeps its end, drops control characters, and never splits a
/// character.
#[test]
fn a_check_s_output_is_kept_within_its_record() {
    let noisy = format!("{}\u{0}\u{1b}[31mé\"the end\"", "é\"".repeat(4096));

    let kept = super::super::kept_output(&noisy);

    let escaped = serde_json::to_string(&kept).expect("json");
    assert!(escaped.len() <= MAX_CHECK_RESULT_OUTPUT_BYTES + 2);
    assert!(kept.ends_with("[31mé\"the end\""));
    assert!(!kept.chars().any(|c| c.is_control()));
    assert!(kept.starts_with('é') || kept.starts_with('"'));
}

/// A Step whose checks the factory ran before never leaves a later round to
/// the agent, even when its session runs no command now: the round is
/// retried, and the agent is never asked to run the checks.
#[tokio::test]
async fn a_step_verified_before_never_falls_back_to_the_agent() {
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let first = verifying(vec![ran(1, "web: 2 failed")], vec![Err(FAILED)]);
    let failed = run_in_session(&first, &github, &repo, input).await;
    let reporter = ScriptedAgent::new(
        "done",
        vec![
            Ok(("done", "Fixed it")),
            Ok(("passed", "Everything passes.")),
        ],
    );

    let retried = run_in_session(&reporter, &github, &repo, input).await;

    assert!(failed.is_err());
    assert!(matches!(
        retried,
        Err(super::super::super::BuiltInStepError::Agent {
            retryable: true,
            ..
        })
    ));
    let turns = reporter.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].outcomes, [Outcome::done()]);
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 2);
    assert!(rounds[1].end.is_none());
}

/// The merge base GitHub gives of the Run's base and the pushed commit.
const BASE: &str = "3333333333333333333333333333333333333333";

/// What the agent's machine was sent: the commands that set the round's
/// worktree up, and those that ran its checks, whole.
fn sent(agent: &ScriptedAgent) -> (Vec<String>, Vec<String>) {
    let set_ups = agent.set_ups.lock().expect("set ups").clone();
    let checks = agent
        .commands
        .lock()
        .expect("commands")
        .iter()
        .map(|command| command.command.command.clone())
        .collect();
    (set_ups, checks)
}

/// Each check runs told the change: `MACRO_CHECK_BASE`, the merge base of
/// the Run's base branch and the pushed commit, and `MACRO_CHECK_CHANGED`,
/// the file beside the worktree that setting it up wrote, listing the
/// paths GitHub compared under a first line that says the list is
/// complete. Both are exported in the check's shell, outside the check's
/// own command, which the round keeps as written.
#[tokio::test]
async fn each_check_is_told_the_change_s_base_and_paths() {
    let agent = verifying(vec![ran(0, "12 passed"), ran(0, "ok")], Vec::new());
    let github = github(Some(CHECKS), Some(BOTH));
    github.state().check_base = Some(BASE.to_string());
    let repo = FakeRepo::default();
    let input = input();

    let report = run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done(), "{}", report.output);
    assert_eq!(
        github.state().compared_changes,
        [("main".to_string(), HEAD.to_string())]
    );
    let (set_ups, checks) = sent(&agent);
    assert_eq!(set_ups.len(), 1);
    assert!(
        set_ups[0].contains(&format!("check_base='{BASE}'\n")),
        "{}",
        set_ups[0]
    );
    assert!(
        set_ups[0].contains(
            "printf '%s\\n' 'complete' 'apps/web/src/main.ts' 'crates/app/src/lib.rs' > \"$worktree.changed.new\" && mv -f \"$worktree.changed.new\" \"$worktree.changed\""
        ),
        "{}",
        set_ups[0]
    );
    assert_eq!(checks.len(), 2);
    for check in &checks {
        assert!(
            check.contains(&format!(
                "\nexport MACRO_CHECK_BASE='{BASE}' MACRO_CHECK_CHANGED=\"$worktree.changed\"\ntimeout "
            )),
            "{check}"
        );
    }
    assert_eq!(
        commands(&agent),
        ["bun run check", "cargo test -p app"],
        "the check's own command is unchanged"
    );
}

/// A Benchmark trial's Run is compared against the commit it is pinned
/// to, not its base branch's head.
#[tokio::test]
async fn a_pinned_run_is_compared_against_its_commit() {
    let agent = verifying(vec![ran(0, "12 passed"), ran(0, "ok")], Vec::new());
    let github = github(Some(CHECKS), Some(BOTH));
    let repo = FakeRepo::default();
    let input = StepInput {
        base_commit: Some(BASE),
        ..input()
    };

    run_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(
        github.state().compared_changes,
        [(BASE.to_string(), HEAD.to_string())]
    );
}

/// A list GitHub gave in part says so on the file's first line.
#[tokio::test]
async fn a_list_github_gave_in_part_says_so() {
    let agent = verifying(vec![ran(0, "12 passed"), ran(0, "ok")], Vec::new());
    let github = github(Some(CHECKS), None);
    github.state().check_base = Some(BASE.to_string());
    let repo = FakeRepo::default();

    run_in_session(&agent, &github, &repo, input())
        .await
        .expect("write");

    let (set_ups, _) = sent(&agent);
    assert!(
        set_ups[0].contains("printf '%s\\n' 'partial' > \"$worktree.changed.new\""),
        "{}",
        set_ups[0]
    );
}

/// A comparison that fails holds no check up: the checks run, told the
/// list is unavailable, with an empty base.
#[tokio::test]
async fn a_failed_comparison_runs_the_checks_told_the_list_is_unavailable() {
    let agent = verifying(vec![ran(0, "12 passed")], Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    github.state().compare_fails = true;
    let repo = FakeRepo::default();

    let report = run_in_session(&agent, &github, &repo, input())
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done(), "{}", report.output);
    let (set_ups, checks) = sent(&agent);
    assert!(set_ups[0].contains("check_base=''\n"), "{}", set_ups[0]);
    assert!(
        set_ups[0].contains("printf '%s\\n' 'unavailable' > "),
        "{}",
        set_ups[0]
    );
    assert!(
        checks[0].contains("export MACRO_CHECK_BASE='' MACRO_CHECK_CHANGED="),
        "{}",
        checks[0]
    );
}
