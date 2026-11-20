//! The `accept` Step: a verifier checks the change, and the plan's
//! acceptance checks run as a round of checks at the commit under check.
//! And the `plan` Step that writes them.

use chrono::Utc;
use factory_records::domain::models::{Step, StepEnd};

use super::*;
use crate::domain::verification::VERIFY_FILE;

/// A plan whose acceptance checks are two tests and a clause with none.
const PLAN: &str = r#"Add the toggle.

```json
{"acceptance": [
  {"clause": "The toggle switches the theme", "run": "bun test toggle", "timeout": 120},
  {"clause": "The page loads", "run": "bun test page"},
  {"clause": "It looks right on a phone", "none": "only a person on a phone sees it"}
]}
```"#;

/// The Run's earlier Steps: its plan, as the `plan` Step keeps it.
fn planned(output: &str) -> Vec<Step> {
    recorded(&crate::domain::acceptance::plan_output(output))
}

/// The Run's earlier Steps: a plan Step that ended with `output`, as it was
/// recorded.
fn recorded(output: &str) -> Vec<Step> {
    vec![Step {
        seq: 1,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: output.to_string(),
            ended_at: Utc::now(),
        }),
        gate: None,
        checks: Vec::new(),
        terminal: Default::default(),
    }]
}

/// [`run`], for a Step whose verifier works in a session it claimed, as a
/// real agent's does: the factory then runs the checks in it.
async fn run(
    agent: &ScriptedAgent,
    github: &FakeGithub,
    repo: &FakeRepo,
    step: &str,
    input: StepInput<'_>,
) -> Result<StepReport, super::super::BuiltInStepError> {
    run_visit(agent, github, repo, step, input, 1).await
}

/// [`run`], at the entry's visit `visit`.
async fn run_visit(
    agent: &ScriptedAgent,
    github: &FakeGithub,
    repo: &FakeRepo,
    step: &str,
    input: StepInput<'_>,
    visit: u16,
) -> Result<StepReport, super::super::BuiltInStepError> {
    let entry = EntryId::try_from(step.to_string()).expect("valid");
    if repo
        .claimed_session(input.key)
        .await
        .expect("read")
        .is_none()
    {
        repo.start_step(input.key, &entry, visit)
            .await
            .expect("start");
        repo.claim_session(
            input.key,
            StepSession {
                session_id: Uuid::now_v7(),
                action_id: Uuid::now_v7(),
                agent: Some(AgentName::try_from("primary".to_string()).expect("valid")),
                bot: Some(Uuid::now_v7()),
            },
            &Role::try_from("verifier".to_string()).expect("valid"),
            &[Outcome::done()],
        )
        .await
        .expect("claim");
    }
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let step = StepName::try_from(step.to_string()).expect("valid");
    steps.run(&step, input).await
}

fn ran(exit_code: i32, stdout: &str) -> Ran {
    Ok(SessionCommandOutput {
        exit_code,
        stdout: stdout.to_string(),
        stderr: String::new(),
    })
}

/// A verifier that ends its check with `verdict`, in a session whose
/// machine runs commands, each ending as `runs` say in turn; set up at
/// `commit`.
fn verifier(verdict: &'static str, runs: Vec<Ran>, commit: &str) -> ScriptedAgent {
    ScriptedAgent {
        work_text: "Each clause holds.".to_string(),
        checkout: Some(Arc::new(Mutex::new(runs.into()))),
        set_up: Some(ran(0, &format!("macro-check: worktree at {commit}\n"))),
        ..ScriptedAgent::new(verdict, Vec::new())
    }
}

/// The checks the machine ran, by their commands.
fn commands(agent: &ScriptedAgent) -> Vec<String> {
    agent
        .commands
        .lock()
        .expect("commands")
        .iter()
        .map(|command| super::run_of(&command.command.command))
        .collect()
}

/// GitHub with the Run's branch at [`HEAD`], its base declaring `verify`
/// as its verify file.
fn declaring(verify: Option<&str>) -> FakeGithub {
    let github = github(None, None);
    if let Some(verify) = verify {
        github.state().files.insert(
            ("main".to_string(), VERIFY_FILE.to_string()),
            verify.to_string(),
        );
    }
    github
}

/// Before the merge, the factory runs the plan's checks on the head of
/// the Run's branch, told the commit, and the
/// verifier, an agent of its own Role, checks the Done-when clauses:
/// both pass, so the Step does.
#[tokio::test]
async fn accept_runs_the_plan_s_checks_on_the_branch_and_passes() {
    let agent = verifier("pass", vec![ran(0, "1 passed"), ran(0, "2 passed")], HEAD);
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "pass", "{}", report.output);
    let commands = commands(&agent);
    assert_eq!(commands.len(), 2, "{commands:?}");
    for (command, run) in commands.iter().zip(["bun test toggle", "bun test page"]) {
        assert!(
            command.starts_with(&format!("export MACRO_VERIFY_COMMIT={HEAD}; "))
                && command.ends_with(run),
            "{command}"
        );
    }
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 1);
    assert!(rounds[0].verified);
    assert_eq!(rounds[0].checks, ["acceptance-1", "acceptance-2"]);
    // The round keeps and shows the command as the plan wrote it.
    assert_eq!(rounds[0].commands, ["bun test toggle", "bun test page"]);
    assert_eq!(rounds[0].checked_commit.as_deref(), Some(HEAD));

    let tasks = agent.tasks.lock().expect("tasks");
    assert_eq!(tasks[0].role.as_str(), "verifier");
    let prompt = &tasks[0].prompt;
    assert!(prompt.contains("# Verify the change"), "{prompt}");
    assert!(prompt.contains("It is not merged yet"), "{prompt}");
    assert!(
        prompt.contains("1. The toggle switches the theme: the factory runs `acceptance-1`"),
        "{prompt}"
    );
    assert!(
        prompt.contains("2. The page loads: the factory runs `acceptance-2`"),
        "{prompt}"
    );
    assert!(
        prompt.contains(
            "3. It looks right on a phone: check it by hand; only a person on a phone sees it"
        ),
        "{prompt}"
    );
    assert!(
        prompt.contains("confirm it really exercises its clause"),
        "{prompt}"
    );
    assert!(agent.turns().is_empty());
    assert!(
        report
            .output
            .contains("- It looks right on a phone: only a person on a phone sees it"),
        "{}",
        report.output
    );
}

/// A check that fails before the merge sends the change back as
/// `blockers`, whatever the verifier found, with the check's output.
#[tokio::test]
async fn a_failed_acceptance_check_before_the_merge_goes_back_as_blockers() {
    let agent = verifier(
        "pass",
        vec![ran(1, "expected dark, got light"), ran(0, "ok")],
        HEAD,
    );
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "blockers");
    assert!(
        report.output.contains("acceptance checks failed"),
        "{}",
        report.output
    );
    assert!(
        report.output.contains("expected dark, got light"),
        "{}",
        report.output
    );
}

/// A verifier that stopped without a result, even once reminded, ends its
/// Step `no_result` with its last message, for the workflow to ask the
/// Run's owner: it is no verdict, so no acceptance check runs and the Step
/// does not go back as `blockers`.
#[tokio::test]
async fn a_verifier_that_stopped_without_a_result_asks_its_owner() {
    let agent = verifier("stopped", vec![ran(0, "1 passed")], HEAD);
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome, Outcome::no_result(), "{}", report.output);
    assert_eq!(report.output, "I stopped before checking.");
    assert!(commands(&agent).is_empty());
    assert!(rounds(&repo, input).await.is_empty());
}

/// At the `accept` entry's last visit, a failure ends `unresolved`, so the
/// Run is left for a person rather than sent to a fix no acceptance would
/// check.
#[tokio::test]
async fn acceptance_that_keeps_failing_is_unresolved_at_the_last_visit() {
    let agent = verifier("fail", vec![ran(0, "ok"), ran(0, "ok")], HEAD);
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run_visit(&agent, &github, &repo, "accept", input, 3)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "unresolved", "{}", report.output);
    assert!(
        report.output.starts_with("Acceptance kept failing"),
        "{}",
        report.output
    );
}

/// Where the verifier's session runs no command, it runs the checks itself
/// at the commit under check, and reports how they went.
#[tokio::test]
async fn where_the_session_runs_no_command_the_verifier_runs_the_checks() {
    let agent = ScriptedAgent::new("pass", vec![Ok(("passed", "acceptance-1: exit 0"))]);
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "pass", "{}", report.output);
    let turns = agent.turns();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].role.as_str(), "verifier");
    assert!(
        turns[0].prompt.contains(&format!("The commit: {HEAD}.")),
        "{}",
        turns[0].prompt
    );
    for run in ["bun test toggle", "bun test page"] {
        assert!(turns[0].prompt.contains(run), "{}", turns[0].prompt);
    }
    assert!(!rounds(&repo, input).await[0].verified);
    assert!(
        report.output.contains("run by the verifier"),
        "{}",
        report.output
    );
}

/// A repository that turns the Phase off passes the change on unchecked,
/// one whose file still names the deploy an earlier factory waited for
/// too.
#[tokio::test]
async fn a_repository_that_turns_the_phase_off_is_not_verified() {
    for file in [
        r#"{"verify": false}"#,
        r#"{"verify": false, "deploy": {"environment": "production"}}"#,
    ] {
        let agent = verifier("fail", Vec::new(), HEAD);
        let github = declaring(Some(file));
        let repo = FakeRepo::default();
        let earlier = planned(PLAN);
        let input = StepInput {
            earlier: &earlier,
            ..input()
        };

        let report = run(&agent, &github, &repo, "accept", input)
            .await
            .expect("off");
        assert_eq!(report.outcome.as_str(), "off", "{file}");
        assert!(agent.tasks.lock().expect("tasks").is_empty(), "{file}");
    }
}

/// A plan with no acceptance checks leaves the verifier to check the
/// Done-when clauses by hand: its word decides.
#[tokio::test]
async fn with_no_acceptance_checks_the_verifier_decides() {
    let github = declaring(None);
    let repo = FakeRepo::default();
    let earlier = planned("Add the toggle.");
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let agent = verifier("pass", Vec::new(), HEAD);
    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "pass");
    assert!(rounds(&repo, input).await.is_empty());
    let prompt = &agent.tasks.lock().expect("tasks")[0].prompt;
    assert!(
        prompt.contains("The plan wrote no acceptance checks"),
        "{prompt}"
    );
}

/// A retried Step goes on with the round it recorded: the checks that ran
/// do not run again.
#[tokio::test]
async fn a_retried_step_goes_on_with_its_round() {
    let agent = verifier("pass", vec![ran(1, "failed once"), ran(0, "ok")], HEAD);
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };
    let first = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");
    let again = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept again");

    assert_eq!(again.outcome, first.outcome);
    assert_eq!(commands(&agent).len(), 2);
    assert_eq!(rounds(&repo, input).await.len(), 1);
}

/// On a repository, the planner is asked for acceptance checks, and the
/// plan the Step keeps shows them for the person at its Gate, with the
/// block the later Steps read back. A plan on no repository is kept as
/// written.
#[tokio::test]
async fn a_plan_on_a_repository_keeps_its_acceptance_checks() {
    let agent = ScriptedAgent {
        work_text: PLAN.to_string(),
        ..ScriptedAgent::new("done", Vec::new())
    };
    let repo = FakeRepo::default();

    let report = run(&agent, &github(None, None), &repo, "plan", input())
        .await
        .expect("plan");

    assert!(
        report.output.contains("## Acceptance checks"),
        "{}",
        report.output
    );
    assert!(
        report
            .output
            .contains("1. The toggle switches the theme: checked by `bun test toggle`"),
        "{}",
        report.output
    );
    let checks = crate::domain::acceptance::AcceptanceChecks::from_plan(&report.output)
        .expect("reads")
        .expect("kept");
    assert_eq!(checks.clauses.len(), 3);
    assert!(
        agent.tasks.lock().expect("tasks")[0]
            .prompt
            .contains("End the plan with a fenced `json` block of acceptance checks")
    );
    // The planner is told what each check is told, and that the checks
    // run before the merge; nothing runs after it, so no deploy is named.
    let prompt = agent.tasks.lock().expect("tasks")[0].prompt.clone();
    assert!(
        prompt.contains("`$MACRO_VERIFY_COMMIT` names the commit under check"),
        "{prompt}"
    );
    assert!(
        prompt.contains("Before the merge, the factory runs every check on the run's branch"),
        "{prompt}"
    );
    assert!(!prompt.contains("deploy"), "{prompt}");
    assert!(!prompt.contains("does not read"), "{prompt}");

    let no_repository = StepInput {
        repository: None,
        ..input()
    };
    let report = run(&agent, &github(None, None), &repo, "plan", no_repository)
        .await
        .expect("plan");
    assert_eq!(report.output, PLAN);
    assert!(
        !agent.tasks.lock().expect("tasks")[1]
            .prompt
            .contains("acceptance checks")
    );
}

/// A plan recorded before Runs ended at their merge may mark a check
/// `deployed`: nothing deploys the change now, so `accept` never runs that
/// check, which would fail, and lists its clause for the verifier to check
/// by hand. The other check runs.
#[tokio::test]
async fn a_recorded_plan_s_deployed_check_is_checked_by_hand() {
    let agent = verifier(
        "pass",
        vec![ran(0, "ok"), ran(7, "MACRO_DEPLOY_URL is not set")],
        HEAD,
    );
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier = recorded(
        r#"Add the toggle.

```json
{"acceptance": [
  {"clause": "The toggle switches the theme", "run": "bun test toggle"},
  {"clause": "The live page serves it", "run": "curl -f \"$MACRO_DEPLOY_URL/toggle\"", "deployed": true}
]}
```"#,
    );
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "pass", "{}", report.output);
    let commands = commands(&agent);
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert!(commands[0].ends_with("bun test toggle"), "{}", commands[0]);
    assert_eq!(rounds(&repo, input).await[0].checks, ["acceptance-1"]);
    let hand = "the plan's check `curl -f \"$MACRO_DEPLOY_URL/toggle\"` needed the deployed change";
    assert!(
        report
            .output
            .contains(&format!("- The live page serves it: {hand}")),
        "{}",
        report.output
    );
    assert!(!report.output.contains("did not read"), "{}", report.output);
    // The verifier is told the command the check ran, and that the missing
    // deployment does not fail the clause.
    let prompt = agent.tasks.lock().expect("tasks")[0].prompt.clone();
    assert!(
        prompt.contains(&format!(
            "2. The live page serves it: check it by hand; {hand}"
        )),
        "{prompt}"
    );
    assert!(
        prompt.contains("that it cannot be seen deployed does not fail it"),
        "{prompt}"
    );
    assert!(
        prompt.contains("That it cannot be seen deployed does not fail it."),
        "{prompt}"
    );
}

/// Acceptance checks that do not read leave every clause to the verifier,
/// and the Step's report says so.
#[tokio::test]
async fn checks_that_do_not_read_are_reported() {
    let agent = verifier("pass", Vec::new(), HEAD);
    let github = github(None, None);
    let repo = FakeRepo::default();
    let earlier =
        recorded("Add the toggle.\n\n```json\n{\"acceptance\": [{\"clause\": \"A\"}]}\n```");
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "pass", "{}", report.output);
    assert!(rounds(&repo, input).await.is_empty());
    assert!(
        report
            .output
            .contains("The plan's acceptance checks do not read (clause 1 of the acceptance block"),
        "{}",
        report.output
    );
}

/// A verify file that does not read, such as one with a key it does not
/// know, turns nothing off: the change is checked, and both the Step's
/// report and the planner say why the file was not followed.
#[tokio::test]
async fn a_verify_file_that_does_not_read_is_reported_and_the_change_checked() {
    let file = r#"{"verify": false, "skip": true}"#;
    let note = "`.macro/verify.json` does not read (unknown field `skip`";
    let agent = verifier("pass", vec![ran(0, "ok"), ran(0, "ok")], HEAD);
    let github = declaring(Some(file));
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let accepting = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", accepting)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "pass", "{}", report.output);
    assert_eq!(commands(&agent).len(), 2);
    assert!(report.output.contains(note), "{}", report.output);
    assert!(
        report.output.contains("): the change is checked."),
        "{}",
        report.output
    );

    let planner = ScriptedAgent {
        work_text: PLAN.to_string(),
        ..ScriptedAgent::new("done", Vec::new())
    };
    run(&planner, &github, &FakeRepo::default(), "plan", input())
        .await
        .expect("plan");
    let prompt = planner.tasks.lock().expect("tasks")[0].prompt.clone();
    assert!(prompt.contains(note), "{prompt}");
    assert!(prompt.contains("): the change is checked."), "{prompt}");
}

/// The base's setup reaches the verifier's session, and its acceptance
/// round keeps it and runs it in the round's worktree before the checks.
#[tokio::test]
async fn accept_runs_the_base_s_setup_before_the_plan_s_checks() {
    let agent = verifier(
        "pass",
        vec![ran(0, "installed"), ran(0, "1 passed"), ran(0, "2 passed")],
        HEAD,
    );
    let github = github(None, None);
    let setup = r#"{"setup":[{"name":"install","run":"bun install"}]}"#;
    github.state().files.insert(
        (
            "main".to_string(),
            crate::domain::setup::SETUP_FILE.to_string(),
        ),
        setup.to_string(),
    );
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let report = run(&agent, &github, &repo, "accept", input)
        .await
        .expect("accept");

    assert_eq!(report.outcome.as_str(), "pass", "{}", report.output);
    assert_eq!(
        commands(&agent),
        [
            "bun install".to_string(),
            format!("export MACRO_VERIFY_COMMIT={HEAD}; bun test toggle"),
            format!("export MACRO_VERIFY_COMMIT={HEAD}; bun test page"),
        ]
    );
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds[0].setup.as_deref(), Some(setup));
    let tasks = agent.tasks.lock().expect("tasks");
    assert!(tasks[0].setup.is_some(), "the verifier's session is set up");
}
