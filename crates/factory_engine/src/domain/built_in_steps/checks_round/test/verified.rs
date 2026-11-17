//! Rounds whose checks the engine runs itself, in the agent's checkout,
//! reading each one's exit code.

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
        .map(|command| command.command.command.clone())
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
        PullRequestSteps::new(github.clone(), repo.clone()),
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
        "The repository's checks passed in round 1, run by the factory in the agent's checkout: web."
    ));
    assert!(agent.turns().is_empty());
    let sent = agent.commands.lock().expect("commands").clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].command.cwd, ".");
    assert_eq!(sent[0].command.command, "bun run check");
    assert_eq!(sent[0].role.as_str(), "writer");
    let rounds = rounds(&repo, input).await;
    assert!(rounds[0].verified);
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
        "The repository's checks still fail in round 2, run by the factory in the agent's checkout, their last: web."
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
