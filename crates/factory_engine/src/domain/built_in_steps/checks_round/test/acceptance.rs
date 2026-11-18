//! The `accept` and `verify` Steps: a verifier checks the change, and the
//! plan's acceptance checks run as a round of checks at the commit under
//! check. And the `plan` Step that writes them.

use chrono::Utc;
use factory_records::domain::models::{Step, StepEnd};

use super::*;
use crate::domain::models::{Deployment, DeploymentState};
use crate::domain::verification::VERIFY_FILE;

/// A plan whose acceptance checks are a test, a check of the deployed
/// change, and a clause with none.
const PLAN: &str = r#"Add the toggle.

```json
{"acceptance": [
  {"clause": "The toggle switches the theme", "run": "bun test toggle", "timeout": 120},
  {"clause": "The live page serves it", "run": "curl -f \"$MACRO_DEPLOY_URL/toggle\"", "deployed": true},
  {"clause": "It looks right on a phone", "none": "only a person on a phone sees it"}
]}
```"#;

/// The Run's earlier Steps: its plan, as the `plan` Step keeps it.
fn planned(output: &str) -> Vec<Step> {
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
            output: crate::domain::acceptance::plan_output(output),
            ended_at: Utc::now(),
        }),
        gate: None,
        checks: Vec::new(),
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
    let entry = EntryId::try_from(step.to_string()).expect("valid");
    if repo.claimed_session(input.key).await.expect("read").is_none() {
        repo.start_step(input.key, &entry, 1).await.expect("start");
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

/// GitHub with the Run's branch at [`HEAD`] and its pull request merged
/// as [`FakeGithub::MERGED`], its base declaring `verify` as its verify
/// file.
fn merged(verify: Option<&str>) -> FakeGithub {
    let github = github(None, None);
    github.state().pull_request = Some(crate::domain::models::PullRequest {
        number: 7,
        url: "https://github.com/macro/app/pull/7".to_string(),
        state: crate::domain::models::PullRequestState::Merged,
        head_sha: HEAD.to_string(),
        merge_commit: Some(FakeGithub::MERGED.to_string()),
    });
    if let Some(verify) = verify {
        github.state().files.insert(
            ("main".to_string(), VERIFY_FILE.to_string()),
            verify.to_string(),
        );
    }
    github
}

/// Before the merge, the factory runs the plan's checks that need no
/// deploy on the head of the Run's branch, told the commit, and the
/// verifier, an agent of its own Role, checks the Done-when clauses:
/// both pass, so the Step does.
#[tokio::test]
async fn accept_runs_the_plan_s_checks_on_the_branch_and_passes() {
    let agent = verifier("pass", vec![ran(0, "1 passed")], HEAD);
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
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert!(
        commands[0].starts_with(&format!("export MACRO_VERIFY_COMMIT={HEAD}; "))
            && commands[0].ends_with("bun test toggle"),
        "{}",
        commands[0]
    );
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 1);
    assert!(rounds[0].verified);
    assert_eq!(rounds[0].checks, ["acceptance-1"]);
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
    assert!(prompt.contains("2. The live page serves it: checked once deployed"), "{prompt}");
    assert!(
        prompt.contains("3. It looks right on a phone: no check (only a person on a phone sees it)"),
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
    let agent = verifier("pass", vec![ran(1, "expected dark, got light")], HEAD);
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
    assert!(report.output.contains("acceptance checks failed"), "{}", report.output);
    assert!(report.output.contains("expected dark, got light"), "{}", report.output);
}

/// After the merge, every check runs on the merge commit, told where the
/// change is deployed; a verifier that finds a clause not met fails the
/// Step, though every check passed.
#[tokio::test]
async fn a_merged_change_is_checked_where_it_is_deployed() {
    let github = merged(Some(r#"{"deploy": {"environment": "production"}}"#));
    github.state().deployments = [Some(Deployment {
        state: DeploymentState::Succeeded,
        url: Some("https://app.example.com".to_string()),
    })]
    .into();
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let agent = verifier("fail", vec![ran(0, "ok"), ran(0, "ok")], FakeGithub::MERGED);
    let report = run(&agent, &github, &repo, "verify", input)
        .await
        .expect("verify");

    assert_eq!(report.outcome.as_str(), "fail", "{}", report.output);
    assert!(
        report
            .output
            .contains("the verifier found a Done-when clause not met"),
        "{}",
        report.output
    );
    let commands = commands(&agent);
    assert_eq!(commands.len(), 2, "{commands:?}");
    assert!(
        commands[1].contains("export MACRO_DEPLOY_URL='https://app.example.com'")
            && commands[1].contains(FakeGithub::MERGED),
        "{}",
        commands[1]
    );
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds[0].checks, ["acceptance-1", "acceptance-2"]);
    assert_eq!(rounds[0].checked_commit.as_deref(), Some(FakeGithub::MERGED));
    let prompt = &agent.tasks.lock().expect("tasks")[0].prompt;
    assert!(
        prompt.contains("deployed to production at https://app.example.com"),
        "{prompt}"
    );
    assert!(prompt.contains("It is merged: nothing goes back"), "{prompt}");
}

/// A merged change whose check fails fails the Step.
#[tokio::test]
async fn a_failed_check_after_the_merge_fails_the_step() {
    let github = merged(None);
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let agent = verifier("pass", vec![ran(0, "ok"), ran(7, "404")], FakeGithub::MERGED);
    let report = run(&agent, &github, &repo, "verify", input)
        .await
        .expect("verify");

    assert_eq!(report.outcome.as_str(), "fail", "{}", report.output);
    assert!(report.output.contains("acceptance checks failed"));
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
    assert!(turns[0].prompt.contains(&format!("The commit: {HEAD}.")), "{}", turns[0].prompt);
    assert!(turns[0].prompt.contains("bun test toggle"), "{}", turns[0].prompt);
    assert!(!rounds(&repo, input).await[0].verified);
    assert!(report.output.contains("run by the verifier"), "{}", report.output);
}

/// A repository that turns the Phase off passes the change on unchecked.
#[tokio::test]
async fn a_repository_that_turns_the_phase_off_is_not_verified() {
    let agent = verifier("fail", Vec::new(), HEAD);
    let github = merged(Some(r#"{"verify": false}"#));
    let repo = FakeRepo::default();
    let earlier = planned(PLAN);
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    for step in ["accept", "verify"] {
        let report = run(&agent, &github, &repo, step, input)
            .await
            .expect("off");
        assert_eq!(report.outcome.as_str(), "off", "{step}");
    }
    assert!(agent.tasks.lock().expect("tasks").is_empty());
}

/// A plan with no acceptance checks leaves the verifier to check the
/// Done-when clauses by hand: its word decides.
#[tokio::test]
async fn with_no_acceptance_checks_the_verifier_decides() {
    let github = merged(None);
    let repo = FakeRepo::default();
    let earlier = planned("Add the toggle.");
    let input = StepInput {
        earlier: &earlier,
        ..input()
    };

    let agent = verifier("pass", Vec::new(), FakeGithub::MERGED);
    let report = run(&agent, &github, &repo, "verify", input)
        .await
        .expect("verify");

    assert_eq!(report.outcome.as_str(), "pass");
    assert!(rounds(&repo, input).await.is_empty());
    let prompt = &agent.tasks.lock().expect("tasks")[0].prompt;
    assert!(prompt.contains("The plan wrote no acceptance checks"), "{prompt}");
}

/// A retried Step goes on with the round it recorded: the checks that ran
/// do not run again.
#[tokio::test]
async fn a_retried_step_goes_on_with_its_round() {
    let agent = verifier("pass", vec![ran(1, "failed once")], HEAD);
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
    assert_eq!(commands(&agent).len(), 1);
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

    assert!(report.output.contains("## Acceptance checks"), "{}", report.output);
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
