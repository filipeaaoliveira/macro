use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};

use factory_records::domain::models::{
    CheckRound, CheckRoundOutcome, Outcome, Repository, RunId, StepKey,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;

use super::super::BuiltInSteps;
use crate::domain::checks::CHECKS_FILE;
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::flow::StepName;
use crate::domain::models::{AgentAnswer, AgentTask, AgentTurn, StepInput, StepReport};
use crate::domain::ports::{Retryable, StepAgent, StepExecutor};
use crate::domain::pull_request_steps::PullRequestSteps;

#[derive(Debug, thiserror::Error)]
#[error("the agent's session failed")]
struct SessionFailed;

impl Retryable for SessionFailed {
    fn retryable(&self) -> bool {
        true
    }
}

/// Ends its first prompt with `work`, and each later prompt with the next
/// of `later`, keeping every task and later prompt it is given.
#[derive(Clone, Default)]
struct ScriptedAgent {
    work: &'static str,
    later: Arc<Mutex<VecDeque<Result<(&'static str, &'static str), SessionFailed>>>>,
    tasks: Arc<Mutex<Vec<AgentTask>>>,
    turns: Arc<Mutex<Vec<AgentTurn>>>,
}

impl ScriptedAgent {
    fn new(work: &'static str, later: Vec<Result<(&'static str, &'static str), SessionFailed>>) -> Self {
        Self {
            work,
            later: Arc::new(Mutex::new(later.into())),
            ..Self::default()
        }
    }

    fn turns(&self) -> Vec<AgentTurn> {
        self.turns.lock().expect("turns").clone()
    }
}

impl StepAgent for ScriptedAgent {
    type Err = SessionFailed;

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, SessionFailed> {
        self.tasks.lock().expect("tasks").push(task);
        Ok(AgentAnswer {
            outcome: outcome(self.work),
            text: "Wrote the toggle".to_string(),
        })
    }

    async fn follow_up(&self, turn: AgentTurn) -> Result<AgentAnswer, SessionFailed> {
        self.turns.lock().expect("turns").push(turn);
        let (ended, text) = self
            .later
            .lock()
            .expect("later")
            .pop_front()
            .expect("a scripted answer")?;
        Ok(AgentAnswer {
            outcome: outcome(ended),
            text: text.to_string(),
        })
    }

    async fn stop(
        &self,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), SessionFailed> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), SessionFailed> {
        Ok(())
    }
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

static REPOSITORY: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/app".to_string(),
    base_branch: "main".to_string(),
});

const CHECKS: &str = r#"{
    "rounds": 2,
    "checks": [
        { "name": "web", "run": "bun run check", "paths": ["apps/web"] },
        { "name": "rust", "run": "cargo test -p app", "paths": ["crates/app"] }
    ]
}"#;

/// A Run's repository on GitHub, its branch declaring `checks` (none when
/// absent) and changing `changed` (unknown when absent).
fn github(checks: Option<&str>, changed: Option<&[&str]>) -> FakeGithub {
    let github = FakeGithub::default();
    if let Some(checks) = checks {
        github
            .state()
            .files
            .insert(CHECKS_FILE.to_string(), checks.to_string());
    }
    github.state().changed =
        changed.map(|paths| paths.iter().map(|path| path.to_string()).collect());
    github
}

/// A Step of a new Run on [`REPOSITORY`].
fn input() -> StepInput<'static> {
    StepInput {
        key: StepKey {
            run_id: RunId::generate(),
            seq: 2,
        },
        owner: &JULIA,
        request: "Add a dark mode toggle",
        task_id: None,
        repository: Some(&REPOSITORY),
        earlier: &[],
    }
}

async fn run(
    agent: &ScriptedAgent,
    github: &FakeGithub,
    repo: &FakeRepo,
    step: &str,
    input: StepInput<'_>,
) -> Result<StepReport, super::BuiltInStepError> {
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone()),
    );
    let step = StepName::try_from(step.to_string()).expect("valid");
    steps.run(&step, input).await
}

async fn rounds(repo: &FakeRepo, input: StepInput<'_>) -> Vec<CheckRound> {
    repo.check_rounds(input.key).await.expect("rounds")
}

/// The agent runs the checks the branch's changes run, in its session, and
/// the Step ends `done` once they pass.
#[tokio::test]
async fn checks_that_pass_end_the_step_done() {
    let agent = ScriptedAgent::new("done", vec![Ok(("passed", "web: exit 0"))]);
    let github = github(Some(CHECKS), Some(&["apps/web/src/main.ts"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    assert!(report.output.starts_with("Wrote the toggle"));
    assert!(report.output.contains("passed in round 1: web."));
    let turns = agent.turns();
    assert_eq!(turns.len(), 1);
    assert!(turns[0].prompt.contains("`web`: `bun run check`"));
    assert!(!turns[0].prompt.contains("cargo test"));
    assert_eq!(turns[0].outcomes, [outcome("passed"), outcome("failed")]);
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].check_action_id, turns[0].action_id);
    assert_eq!(rounds[0].checks, ["web"]);
    let end = rounds[0].end.as_ref().expect("ended");
    assert_eq!(
        (end.outcome, end.report.as_str()),
        (CheckRoundOutcome::Passed, "web: exit 0")
    );
}

/// A failure goes back to the agent with the checks' report, and the next
/// round checks its fix.
#[tokio::test]
async fn a_failure_goes_back_to_the_agent_and_the_next_round_checks_its_fix() {
    let agent = ScriptedAgent::new(
        "done",
        vec![
            Ok(("failed", "rust: exit 101, test toggle_saves failed")),
            Ok(("done", "Fixed the toggle's saving")),
            Ok(("passed", "rust: exit 0")),
        ],
    );
    let github = github(Some(CHECKS), Some(&["crates/app/src/lib.rs"]));
    let repo = FakeRepo::default();
    let input = input();

    let report = run(&agent, &github, &repo, "fix", input)
        .await
        .expect("fix");

    assert_eq!(report.outcome, Outcome::done());
    assert!(report.output.starts_with("Fixed the toggle's saving"));
    assert!(report.output.contains("passed in round 2: rust."));
    let turns = agent.turns();
    assert_eq!(turns.len(), 3);
    assert!(turns[1].prompt.contains("test toggle_saves failed"));
    assert_eq!(turns[1].outcomes, [Outcome::done()]);
    let rounds = rounds(&repo, input).await;
    let outcomes: Vec<CheckRoundOutcome> = rounds
        .iter()
        .map(|round| round.end.as_ref().expect("ended").outcome)
        .collect();
    assert_eq!(
        outcomes,
        [CheckRoundOutcome::Failed, CheckRoundOutcome::Passed]
    );
    assert_eq!(rounds[0].fix_action_id, Some(turns[1].action_id));
    assert_eq!(rounds[1].check_action_id, turns[2].action_id);
    assert_eq!(rounds[1].work_output, "Fixed the toggle's saving");
}

/// Checks that still fail in the repository's last round end the Step
/// `red`, with the report, and nothing more goes back to the agent.
#[tokio::test]
async fn checks_that_never_pass_end_the_step_red() {
    let agent = ScriptedAgent::new(
        "done",
        vec![
            Ok(("failed", "web: exit 1")),
            Ok(("done", "Tried a fix")),
            Ok(("failed", "web: exit 1 again")),
        ],
    );
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();

    let report = run(&agent, &github, &repo, "write", input())
        .await
        .expect("write");

    assert_eq!(report.outcome, outcome("red"));
    assert!(report.output.contains("still fail after 2 rounds: web."));
    assert!(report.output.contains("web: exit 1 again"));
    assert_eq!(agent.turns().len(), 3);
}

/// A retried Step goes on with the round it was at, under the prompt it
/// recorded: it neither redoes the work nor runs a round twice.
#[tokio::test]
async fn a_retried_step_goes_on_with_its_round() {
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let first = ScriptedAgent::new(
        "done",
        vec![Ok(("failed", "web: exit 1")), Err(SessionFailed)],
    );
    let failed = run(&first, &github, &repo, "write", input).await;
    let retry = ScriptedAgent::new(
        "done",
        vec![Ok(("done", "Fixed it")), Ok(("passed", "web: exit 0"))],
    );

    let report = run(&retry, &github, &repo, "write", input)
        .await
        .expect("write");

    assert!(failed.is_err());
    assert_eq!(report.outcome, Outcome::done());
    assert!(retry.tasks.lock().expect("tasks").is_empty());
    let sent_back = first.turns()[1].action_id;
    assert_eq!(retry.turns()[0].action_id, sent_back);
    assert_eq!(rounds(&repo, input).await.len(), 2);
}

/// Every check runs when GitHub cannot list the branch's changes; a branch
/// that declares no checks, or whose changes run none, ends with the
/// agent's work alone.
#[tokio::test]
async fn which_checks_a_branch_runs() {
    let unknown = ScriptedAgent::new("done", vec![Ok(("passed", "both passed"))]);
    run(&unknown, &github(Some(CHECKS), None), &FakeRepo::default(), "write", input())
        .await
        .expect("write");
    let prompt = &unknown.turns()[0].prompt;
    assert!(prompt.contains("bun run check") && prompt.contains("cargo test -p app"));

    for github in [
        github(None, Some(&["apps/web/index.ts"])),
        github(Some(CHECKS), Some(&["README.md"])),
    ] {
        let agent = ScriptedAgent::new("done", Vec::new());
        let report = run(&agent, &github, &FakeRepo::default(), "write", input())
            .await
            .expect("write");
        assert_eq!(report.outcome, Outcome::done());
        assert_eq!(report.output, "Wrote the toggle");
        assert!(agent.turns().is_empty());
    }
}

/// A checks file that does not read ends the Step `red`, saying why; work
/// that changed nothing runs no checks.
#[tokio::test]
async fn an_unreadable_checks_file_ends_the_step_red() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let broken = github(Some(r#"{"rounds": 0, "checks": []}"#), None);

    let report = run(&agent, &broken, &FakeRepo::default(), "write", input())
        .await
        .expect("write");
    let unchanged = ScriptedAgent::new("unchanged", Vec::new());
    let nothing = run(&unchanged, &broken, &FakeRepo::default(), "write", input())
        .await
        .expect("write");

    assert_eq!(report.outcome, outcome("red"));
    assert!(report.output.contains("asks for 0 rounds"));
    assert_eq!(nothing.outcome, Outcome::unchanged());
    assert!(agent.turns().is_empty() && unchanged.turns().is_empty());
}
