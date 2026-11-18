//! The repository's setup: read from the Run's base, handed to the agent
//! for its sessions, and run in a verified round's worktree before the
//! checks.

use super::*;
use crate::domain::models::run_branch;
use crate::domain::setup::SETUP_FILE;

const SETUP: &str = r#"{
    "setup": [{ "name": "install", "run": "bun install" }],
    "env": { "CACHE_DIR": "/tmp/cache" }
}"#;

/// Have `branch` declare `setup`.
fn declare_setup(github: &FakeGithub, branch: &str, setup: &str) {
    github.state().files.insert(
        (branch.to_string(), SETUP_FILE.to_string()),
        setup.to_string(),
    );
}

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

/// The whole shell each command the round ran in its worktree runs.
fn commands(agent: &ScriptedAgent) -> Vec<String> {
    agent
        .commands
        .lock()
        .expect("commands")
        .iter()
        .map(|command| command.command.command.clone())
        .collect()
}

/// A `write` Step of a Run whose agent works in a session it claimed.
async fn write_in_session(
    agent: &ScriptedAgent,
    github: &FakeGithub,
    repo: &FakeRepo,
    input: StepInput<'_>,
) -> Result<StepReport, super::super::BuiltInStepError> {
    let entry = EntryId::try_from("write".to_string()).expect("valid");
    repo.start_step(input.key, &entry, 1).await.expect("start");
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
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let step = StepName::try_from("write".to_string()).expect("valid");
    steps.run(&step, input).await
}

/// An agent Step on a repository hands its agent the setup its base
/// declares, for each new session to run before its first prompt, and
/// tells the agent what ran and with which values.
#[tokio::test]
async fn the_agent_is_handed_the_setup_of_the_run_s_base() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(None, None);
    declare_setup(&github, "main", SETUP);
    let repo = FakeRepo::default();

    run(&agent, &github, &repo, "write", input())
        .await
        .expect("write");

    let tasks = agent.tasks.lock().expect("tasks");
    let setup = tasks[0].setup.as_ref().expect("the base's setup");
    assert_eq!(setup.setup[0].run, "bun install");
    assert!(
        tasks[0]
            .prompt
            .contains("ran the repository's setup (`.macro/setup.json`) in your checkout before this prompt: install."),
        "{}",
        tasks[0].prompt
    );
    assert!(tasks[0].prompt.contains("export CACHE_DIR='/tmp/cache'"));
}

/// A setup of values alone runs nothing, so the agent is not told one ran;
/// it is given the values in a fence no value can close.
#[tokio::test]
async fn a_setup_of_values_alone_is_told_without_claiming_it_ran() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(None, None);
    declare_setup(&github, "main", r#"{"env":{"GREETING":"```hi````"}}"#);
    let repo = FakeRepo::default();

    run(&agent, &github, &repo, "write", input())
        .await
        .expect("write");

    let tasks = agent.tasks.lock().expect("tasks");
    let prompt = &tasks[0].prompt;
    assert!(!prompt.contains("ran the repository's setup"), "{prompt}");
    assert!(
        prompt.contains(
            "The repository's setup (`.macro/setup.json`) gives these values; run your own commands with them too:\n\n`````sh\nexport GREETING='```hi````'\n`````\n"
        ),
        "{prompt}"
    );
}

/// A setup that only the Run's own branch declares, or changes, is never
/// used: the agent could otherwise choose what runs before it.
#[tokio::test]
async fn a_setup_only_the_run_s_branch_declares_is_not_used() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(None, None);
    let input = input();
    declare_setup(&github, &run_branch(input.key.run_id), SETUP);
    let repo = FakeRepo::default();

    run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");

    let tasks = agent.tasks.lock().expect("tasks");
    assert_eq!(tasks[0].setup, None);
    assert!(!tasks[0].prompt.contains(SETUP_FILE));
}

/// A setup file that does not read ends the Step `failed`, saying why,
/// and no agent is asked.
#[tokio::test]
async fn a_setup_file_that_does_not_read_fails_the_step_saying_why() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(None, None);
    declare_setup(&github, "main", r#"{"setup":[],"archive":[]}"#);
    let repo = FakeRepo::default();

    let report = run(&agent, &github, &repo, "write", input())
        .await
        .expect("the step ends");

    assert_eq!(report.outcome, Outcome::failed());
    assert!(
        report
            .output
            .starts_with("The repository's setup could not run, so the agent did not start: .macro/setup.json does not read"),
        "{}",
        report.output
    );
    assert!(agent.tasks.lock().expect("tasks").is_empty());
}

/// A setup that failed in the agent's session ends the Step `failed` with
/// what the owner needs to see, rather than a failure's fixed output.
#[tokio::test]
async fn a_setup_that_failed_in_the_session_ends_the_step_failed_with_its_output() {
    let agent = ScriptedAgent::new("setup_failed", Vec::new());
    let github = github(None, None);
    declare_setup(&github, "main", SETUP);
    let repo = FakeRepo::default();

    let report = run(&agent, &github, &repo, "plan", input())
        .await
        .expect("the step ends");

    assert_eq!(
        report,
        StepReport {
            outcome: Outcome::failed(),
            output: "`install` (`bun install`) exited with status 1.".to_string(),
        }
    );
}

/// A verified round runs the base's setup in its worktree, under the
/// round's lock, before its checks, and each check runs with the setup's
/// values; the round keeps the setup it ran.
#[tokio::test]
async fn a_verified_round_runs_the_setup_before_its_checks() {
    let agent = verifying(vec![ran(0, "installed"), ran(0, "12 passed")], Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    declare_setup(&github, "main", SETUP);
    let repo = FakeRepo::default();
    let input = input();

    let report = write_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done(), "{}", report.output);
    let ran = commands(&agent);
    assert_eq!(ran.len(), 2);
    assert!(
        ran[0].ends_with("bash --noprofile --norc -c 'export CACHE_DIR='\\''/tmp/cache'\\''\nbun install' 9>&-"),
        "{}",
        ran[0]
    );
    assert!(ran[0].contains("flock -w 60"), "{}", ran[0]);
    assert!(
        ran[1].ends_with("-c 'export CACHE_DIR='\\''/tmp/cache'\\''\nbun run check' 9>&-"),
        "{}",
        ran[1]
    );
    let machine = agent.machine.lock().expect("machine").clone();
    assert_eq!(machine.first().map(String::as_str), Some("set up"));
    assert_eq!(machine.last().map(String::as_str), Some("remove"));
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds[0].setup.as_deref(), Some(SETUP));
    assert_eq!(
        rounds[0]
            .results
            .iter()
            .map(|result| (result.name.as_str(), result.exit_code))
            .collect::<Vec<_>>(),
        [("setup:install", 0), ("web", 0)]
    );
}

/// An attempt that ends after the setup passed leaves it recorded: the
/// retry, in the same worktree, goes on with the checks and never runs the
/// setup again, so a long setup and a long check fit in two attempts.
#[tokio::test]
async fn a_retried_round_does_not_run_its_passed_setup_again() {
    let agent = verifying(
        vec![ran(0, "installed"), Err(FAILED), ran(0, "12 passed")],
        Vec::new(),
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    declare_setup(&github, "main", SETUP);
    let repo = FakeRepo::default();
    let input = input();

    let first = write_in_session(&agent, &github, &repo, input).await;
    assert!(first.is_err(), "the session failed during the check");
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let step = StepName::try_from("write".to_string()).expect("valid");
    let report = steps.run(&step, input).await.expect("retried");

    assert_eq!(report.outcome, Outcome::done(), "{}", report.output);
    let ran = commands(&agent);
    assert_eq!(ran.len(), 3, "{ran:#?}");
    assert!(ran[2].contains("bun run check"), "{}", ran[2]);
}

/// A setup that fails in the round's worktree ends the round failed, as a
/// red check: the checks do not run, and the agent's fix is told the
/// command and the end of its output. The next round runs the same setup.
#[tokio::test]
async fn a_setup_that_fails_in_the_round_is_a_red_check() {
    let agent = verifying(
        vec![
            ran(
                1,
                concat!(
                    "using ghp_",
                    "abcdefghijklmnopqrstuvwxyz0123\nerror: lockfile had changes"
                ),
            ),
            ran(0, "installed"),
            ran(0, "12 passed"),
        ],
        vec![Ok(("done", "Updated the lockfile"))],
    );
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    declare_setup(&github, "main", SETUP);
    let repo = FakeRepo::default();
    let input = input();

    let report = write_in_session(&agent, &github, &repo, input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done(), "{}", report.output);
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 2);
    let first = &rounds[0];
    assert_eq!(
        first
            .results
            .iter()
            .map(|result| (result.name.as_str(), result.exit_code))
            .collect::<Vec<_>>(),
        [("setup:install", 1)]
    );
    let end = first.end.as_ref().expect("ended");
    assert_eq!(end.outcome, CheckRoundOutcome::Failed);
    assert!(
        end.report
            .contains("### setup `install`: `bun install` exited with status 1"),
        "{}",
        end.report
    );
    assert!(end.report.contains("error: lockfile had changes"));
    assert!(!end.report.contains("ghp_"), "{}", end.report);
    assert!(end.report.contains(crate::domain::setup::REDACTED));
    assert!(end.report.contains("So these checks did not run: web."));
    let turns = agent.turns();
    assert_eq!(turns.len(), 1);
    assert!(turns[0].prompt.contains("error: lockfile had changes"));
    assert_eq!(rounds[1].setup.as_deref(), Some(SETUP));
    assert_eq!(commands(&agent).len(), 3);
}

/// A setup file that does not read when the first round reads it ends
/// the Step's checks red, saying why, and records no round.
#[tokio::test]
async fn a_round_whose_setup_does_not_read_ends_red() {
    let agent = verifying(Vec::new(), Vec::new());
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    declare_setup(&github, "main", r#"{"setup":[{"name":"x","run":""}]}"#);
    let repo = FakeRepo::default();
    let input = input();
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );

    let ended = steps
        .first_round(input, "Wrote the toggle")
        .await
        .expect("read")
        .expect("the step ends");

    assert_eq!(ended.outcome, outcome("red"));
    assert!(
        ended.output.starts_with(
            "The repository's checks could not run: .macro/setup.json gives command x a blank run"
        ),
        "{}",
        ended.output
    );
    assert!(rounds(&repo, input).await.is_empty());
}
