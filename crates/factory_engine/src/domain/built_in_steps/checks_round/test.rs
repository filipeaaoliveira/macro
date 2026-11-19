use std::collections::VecDeque;
use std::sync::{Arc, LazyLock, Mutex};

use agent_session::domain::session_command::SessionCommandOutput;
use factory_records::domain::models::{
    AgentName, CheckRound, CheckRoundOutcome, EntryId, HandBack, MAX_OUTPUT_BYTES, Outcome,
    Repository, Role, RunId, StepKey, StepSession,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::super::BuiltInSteps;
use crate::domain::checks::CHECKS_FILE;
use crate::domain::fakes::{FakeGithub, FakeRepo, FakeWatches};
use crate::domain::flow::StepName;
use crate::domain::models::{
    AgentAnswer, AgentCommand, AgentTask, AgentTurn, StepInput, StepReport,
};
use crate::domain::ports::{AgentFailure, Retryable, StepAgent, StepExecutor};
use crate::domain::pull_request_steps::PullRequestSteps;

mod acceptance;
mod setup;
mod verified;

#[derive(Debug, Clone, thiserror::Error)]
#[error("the agent's session failed")]
struct SessionFailed {
    out_of_quota: bool,
    /// The agent's last message, when it stopped without a result.
    last_words: Option<&'static str>,
    /// What the owner is shown, when the repository's setup failed in the
    /// session before its first prompt.
    setup: Option<&'static str>,
}

/// A failure a retry may get past.
const FAILED: SessionFailed = SessionFailed {
    out_of_quota: false,
    last_words: None,
    setup: None,
};

/// The agent's quota ran out: asking again does not help.
const NO_QUOTA: SessionFailed = SessionFailed {
    out_of_quota: true,
    last_words: None,
    setup: None,
};

/// The agent stopped without a result, even once reminded.
const STOPPED: SessionFailed = SessionFailed {
    out_of_quota: false,
    last_words: Some("I stopped before checking."),
    setup: None,
};

/// The repository's setup failed in the session before its first prompt.
const SETUP_FAILED: SessionFailed = SessionFailed {
    out_of_quota: false,
    last_words: None,
    setup: Some("`install` (`bun install`) exited with status 1."),
};

impl Retryable for SessionFailed {
    fn retryable(&self) -> bool {
        !self.out_of_quota && self.last_words.is_none() && self.setup.is_none()
    }
}

impl AgentFailure for SessionFailed {
    fn out_of_quota(&self) -> bool {
        self.out_of_quota
    }

    fn stopped_without_result(&self) -> Option<&str> {
        self.last_words
    }

    fn setup_failed(&self) -> Option<&str> {
        self.setup
    }
}

/// A later prompt's outcome and text, or the session failing.
type Scripted = Result<(&'static str, &'static str), SessionFailed>;

/// Ends its first prompt with `work`, handing back `work_text`, and each
/// later prompt with the next of `later`, keeping every task and later
/// prompt it is given. When `ages` is set, a later prompt that fails takes
/// its Step past its time for rounds, as one that worked too long would.
#[derive(Clone, Default)]
struct ScriptedAgent {
    work: &'static str,
    work_text: String,
    later: Arc<Mutex<VecDeque<Scripted>>>,
    ages: Option<FakeRepo>,
    tasks: Arc<Mutex<Vec<AgentTask>>>,
    turns: Arc<Mutex<Vec<AgentTurn>>>,
    /// How each command run in the session's checkout ends, in order: none
    /// for a runtime that runs no command, as a paired machine's.
    checkout: Option<Arc<Mutex<VecDeque<Ran>>>>,
    commands: Arc<Mutex<Vec<AgentCommand>>>,
    /// How long each command run in the worktree, a check's or a setup
    /// command's, takes.
    run_takes: Option<std::time::Duration>,
    /// What setting a round's worktree up answers: by default, that it is
    /// at the branch's [`HEAD`].
    set_up: Option<Ran>,
    /// What the machine ran, in order: `set up`, `check <its command>` and
    /// `remove`.
    machine: Arc<Mutex<Vec<String>>>,
}

/// The check a check's command runs: its last line is `timeout … bash
/// --noprofile --norc -c '<run>' 9>&-`.
fn run_of(command: &str) -> String {
    let last = command.lines().last().unwrap_or_default();
    let quoted = last
        .split_once(" -c ")
        .map_or("", |(_, rest)| rest)
        .trim_end_matches(" 9>&-");
    quoted.trim_matches('\'').replace(r"'\''", "'")
}

/// The head commit of the Run's branch on GitHub.
const HEAD: &str = "1111111111111111111111111111111111111111";

/// How a command run in the checkout ended, or the session failing.
type Ran = Result<SessionCommandOutput, SessionFailed>;

impl ScriptedAgent {
    fn new(work: &'static str, later: Vec<Scripted>) -> Self {
        Self {
            work,
            work_text: "Wrote the toggle".to_string(),
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
        let key = task.key;
        self.tasks.lock().expect("tasks").push(task);
        if self.work == "slow" {
            if let Some(repo) = &self.ages {
                age(repo, key);
            }
            return Err(FAILED);
        }
        if self.work == "stopped" {
            return Err(STOPPED);
        }
        if self.work == "setup_failed" {
            return Err(SETUP_FAILED);
        }
        Ok(AgentAnswer {
            outcome: outcome(self.work),
            text: self.work_text.clone(),
        })
    }

    async fn follow_up(&self, turn: AgentTurn) -> Result<AgentAnswer, SessionFailed> {
        let key = turn.key;
        self.turns.lock().expect("turns").push(turn);
        let scripted = self
            .later
            .lock()
            .expect("later")
            .pop_front()
            .expect("a scripted answer");
        if scripted.is_err()
            && let Some(repo) = &self.ages
        {
            age(repo, key);
        }
        let (ended, text) = scripted?;
        Ok(AgentAnswer {
            outcome: outcome(ended),
            text: text.to_string(),
        })
    }

    async fn run_check(
        &self,
        command: AgentCommand,
    ) -> Result<Option<SessionCommandOutput>, SessionFailed> {
        let Some(checkout) = &self.checkout else {
            return Ok(None);
        };
        let line = command.command.command.clone();
        if line.contains("g worktree add") {
            self.machine
                .lock()
                .expect("machine")
                .push("set up".to_string());
            return self
                .set_up
                .clone()
                .unwrap_or_else(|| {
                    Ok(SessionCommandOutput {
                        exit_code: 0,
                        stdout: format!("macro-check: worktree at {HEAD}\n"),
                        stderr: String::new(),
                    })
                })
                .map(Some);
        }
        if line.contains("g worktree remove") {
            self.machine
                .lock()
                .expect("machine")
                .push("remove".to_string());
            return Ok(Some(SessionCommandOutput {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            }));
        }
        let run = run_of(&line);
        self.machine
            .lock()
            .expect("machine")
            .push(format!("check {run}"));
        let key = command.key;
        self.commands.lock().expect("commands").push(command);
        if let Some(takes) = self.run_takes {
            tokio::time::sleep(takes).await;
        }
        let ran = checkout
            .lock()
            .expect("checkout")
            .pop_front()
            .expect("a scripted run");
        if ran.is_err()
            && let Some(repo) = &self.ages
        {
            age(repo, key);
        }
        ran.map(Some)
    }

    async fn stop(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), SessionFailed> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), SessionFailed> {
        Ok(())
    }
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).expect("valid")
}

/// Take the Step `key` back to two hours before now: past its time for
/// rounds.
fn age(repo: &FakeRepo, key: StepKey) {
    let mut steps = repo.steps.lock().expect("steps");
    for (run, step) in steps.iter_mut() {
        if *run == key.run_id && step.seq == key.seq {
            step.started_at = chrono::Utc::now() - chrono::TimeDelta::hours(2);
        }
    }
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

/// A Run's repository on GitHub, its base declaring `checks` (none when
/// absent) and its branch changing `changed` (unknown when absent).
fn github(checks: Option<&str>, changed: Option<&[&str]>) -> FakeGithub {
    let github = FakeGithub::default();
    if let Some(checks) = checks {
        declare(&github, "main", checks);
    }
    github.state().changed =
        changed.map(|paths| paths.iter().map(|path| path.to_string()).collect());
    github.state().head = Some(HEAD.to_string());
    github
}

/// Have `branch` declare `checks`.
fn declare(github: &FakeGithub, branch: &str, checks: &str) {
    github.state().files.insert(
        (branch.to_string(), CHECKS_FILE.to_string()),
        checks.to_string(),
    );
}

/// A Step of a new Run on [`REPOSITORY`].
fn input() -> StepInput<'static> {
    StepInput {
        key: StepKey {
            run_id: RunId::generate(),
            seq: 2,
        },
        role: None,
        owner: &JULIA,
        request: "Add a dark mode toggle",
        created_in_person: true,
        control_file_patterns: None,
        definition_repo: None,
        task_id: None,
        repository: Some(&REPOSITORY),
        earlier: &[],
        review: None,
        reviewer: false,
        may_ask: false,
        questions_left: 0,
        resumes: None,
        owner_notes: &[],
        base_commit: None,
        routes: &[],
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
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let entry = EntryId::try_from(step.to_string()).expect("valid");
    repo.start_step(input.key, &entry, 1).await.expect("start");
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
    assert!(
        report
            .output
            .starts_with("The repository's checks passed in round 1: web.")
    );
    assert!(report.output.contains("Wrote the toggle"));
    let turns = agent.turns();
    assert_eq!(turns.len(), 1);
    assert!(turns[0].prompt.contains("`web`: `bun run check`"));
    assert!(!turns[0].prompt.contains("cargo test"));
    assert_eq!(turns[0].outcomes, [outcome("passed"), outcome("failed")]);
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0].check_action_id, turns[0].action_id);
    assert_eq!(rounds[0].checks, ["web"]);
    assert_eq!(rounds[0].commands, ["bun run check"]);
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
    assert!(
        report
            .output
            .starts_with("The repository's checks passed in round 2: rust.")
    );
    // The Step keeps the agent's work, and what its fix did.
    assert!(report.output.contains("Wrote the toggle"));
    assert!(
        report
            .output
            .contains("After round 1 failed: Fixed the toggle's saving")
    );
    let turns = agent.turns();
    assert_eq!(turns.len(), 3);
    // Every later prompt goes to the session of the Agent filling the
    // Step's Role.
    assert!(turns.iter().all(|turn| turn.role.as_str() == "fixer"));
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
    assert!(
        report
            .output
            .contains("still fail in round 2, their last: web.")
    );
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
    let first = ScriptedAgent::new("done", vec![Ok(("failed", "web: exit 1")), Err(FAILED)]);
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

/// An agent whose quota runs out on a round's prompt cannot go on, and no
/// other Agent takes over its session: the open round ends failed, saying
/// so, and the Step ends `red`, not failed.
#[tokio::test]
async fn a_quota_that_runs_out_during_the_checks_ends_the_step_red() {
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let agent = ScriptedAgent::new("done", vec![Err(NO_QUOTA)]);

    let report = run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");
    // A retry whose first answer was lost ends the same way, asking the
    // agent nothing.
    let retry = ScriptedAgent::new("done", Vec::new());
    let again = run(&retry, &github, &repo, "write", input)
        .await
        .expect("write again");

    assert_eq!(again, report);
    assert!(retry.turns().is_empty());
    assert!(retry.tasks.lock().expect("tasks").is_empty());
    assert_eq!(report.outcome, outcome("red"));
    assert!(
        report
            .output
            .starts_with("The agent's quota ran out during the repository's checks")
    );
    assert!(report.output.contains("Wrote the toggle"));
    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 1);
    let end = rounds[0].end.as_ref().expect("ended");
    assert_eq!(end.outcome, CheckRoundOutcome::Failed);
    assert_eq!(
        end.report,
        "The agent's quota ran out during the checks, so it could not run them."
    );
}

/// A quota that runs out on the fix of a failed round ends the Step `red`
/// with that round's report.
#[tokio::test]
async fn a_quota_that_runs_out_on_a_fix_ends_the_step_red() {
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let agent = ScriptedAgent::new("done", vec![Ok(("failed", "web: exit 1")), Err(NO_QUOTA)]);

    let report = run(&agent, &github, &repo, "write", input)
        .await
        .expect("write");
    // A retry whose first answer was lost ends the same way, asking the
    // agent nothing.
    let retry = ScriptedAgent::new("done", Vec::new());
    let again = run(&retry, &github, &repo, "write", input)
        .await
        .expect("write again");

    assert_eq!(again, report);
    assert!(retry.turns().is_empty());
    assert!(retry.tasks.lock().expect("tasks").is_empty());
    assert_eq!(report.outcome, outcome("red"));
    assert!(
        report
            .output
            .starts_with("The agent's quota ran out during the repository's checks")
    );
    assert!(report.output.contains("web: exit 1"));
    assert_eq!(rounds(&repo, input).await.len(), 1);
}

/// Every check runs when GitHub cannot list the branch's changes; a branch
/// that declares no checks, or whose changes run none, ends with the
/// agent's work alone.
#[tokio::test]
async fn which_checks_a_branch_runs() {
    let unknown = ScriptedAgent::new("done", vec![Ok(("passed", "both passed"))]);
    run(
        &unknown,
        &github(Some(CHECKS), None),
        &FakeRepo::default(),
        "write",
        input(),
    )
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

/// The checks file is read from the Run's base, which the agent does not
/// push to: one its branch adds, or changes, counts for nothing.
#[tokio::test]
async fn the_branch_cannot_declare_or_change_its_checks() {
    let agent = ScriptedAgent::new("done", Vec::new());
    let github = github(None, Some(&["apps/web/index.ts"]));
    let input = input();
    declare(&github, &format!("factory/{}", input.key.run_id), CHECKS);

    let report = run(&agent, &github, &FakeRepo::default(), "write", input)
        .await
        .expect("write");

    assert_eq!(report.output, "Wrote the toggle");
    assert!(agent.turns().is_empty());
}

/// Later rounds go by the checks file the first round read, and run the
/// checks it ran, whatever the base or the branch's changes say since.
#[tokio::test]
async fn later_rounds_go_by_what_the_first_round_read() {
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let first = ScriptedAgent::new("done", vec![Ok(("failed", "web: exit 1")), Err(FAILED)]);
    let _ = run(&first, &github, &repo, "write", input).await;
    // Since then the base allows more rounds and declares no checks, and
    // the fix reverted the change that ran `web`.
    declare(&github, "main", r#"{"rounds": 10, "checks": []}"#);
    github.state().changed = Some(vec!["README.md".to_string()]);
    let retry = ScriptedAgent::new(
        "done",
        vec![Ok(("done", "Reverted it")), Ok(("failed", "web: exit 1"))],
    );

    let report = run(&retry, &github, &repo, "write", input)
        .await
        .expect("write");

    let rounds = rounds(&repo, input).await;
    assert_eq!(rounds.len(), 2);
    assert_eq!(rounds[1].checks, ["web"]);
    assert_eq!(rounds[1].commands, ["bun run check"]);
    // Two rounds, as the first round's file said: the Step ends there.
    assert_eq!(report.outcome, outcome("red"));
    assert!(
        report
            .output
            .contains("still fail in round 2, their last: web.")
    );
}

/// Rounds stop short of the Step's time and end it `red`, saying so: before
/// asking anything more, and when an agent is still at work as time runs
/// out.
#[tokio::test]
async fn rounds_out_of_time_end_the_step_red() {
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();
    let mut slow = ScriptedAgent::new("done", vec![Err(FAILED)]);
    slow.ages = Some(repo.clone());

    let report = run(&slow, &github, &repo, "write", input())
        .await
        .expect("write");

    assert_eq!(report.outcome, outcome("red"));
    assert!(
        report
            .output
            .contains("before the step's time ran out, with 1 of their rounds ended: web.")
    );
    assert!(report.output.contains("Wrote the toggle"));
    let turn = &slow.turns()[0];
    let left = turn.answer_by - chrono::Utc::now();
    assert!(left > chrono::TimeDelta::minutes(100) && left <= chrono::TimeDelta::minutes(105));
    // The open round ends failed, so it never reads as running.
    let key = turn.key;
    let rounds = repo.check_rounds(key).await.expect("rounds");
    let end = rounds[0].end.as_ref().expect("the round ended");
    assert_eq!(end.outcome, CheckRoundOutcome::Failed);
    assert!(
        end.report
            .contains("time ran out before the agent reported")
    );
    // The work was waited for until the same time.
    let task_by = slow.tasks.lock().expect("tasks")[0].answer_by;
    assert_eq!(task_by, Some(turn.answer_by));

    // A retry past the time asks nothing more.
    let input = input();
    let failing = ScriptedAgent::new("done", vec![Ok(("failed", "web: exit 1")), Err(FAILED)]);
    let _ = run(&failing, &github, &repo, "write", input).await;
    age(&repo, input.key);
    let retry = ScriptedAgent::new("done", Vec::new());
    let report = run(&retry, &github, &repo, "write", input)
        .await
        .expect("write");
    assert_eq!(report.outcome, outcome("red"));
    assert!(report.output.contains("with 1 of their rounds ended: web."));
    assert!(report.output.contains("web: exit 1"));
    assert!(retry.turns().is_empty());
}

/// An output past what a Step keeps is cut, with a mark, keeping what the
/// checks said first.
#[tokio::test]
async fn an_output_too_long_for_a_step_is_cut() {
    let mut agent = ScriptedAgent::new("done", vec![Ok(("passed", "web: exit 0"))]);
    agent.work_text = "é".repeat(MAX_OUTPUT_BYTES);
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));

    let report = run(&agent, &github, &FakeRepo::default(), "write", input())
        .await
        .expect("write");

    assert!(report.output.len() <= MAX_OUTPUT_BYTES);
    assert!(
        report
            .output
            .starts_with("The repository's checks passed in round 1: web.")
    );
    assert!(report.output.ends_with("a step's output holds 64 KiB.)"));
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

/// Work still going when the Step's time runs out ends the Step `red`,
/// saying the checks did not run, rather than failed.
#[tokio::test]
async fn work_out_of_time_ends_the_step_red() {
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();
    let mut slow = ScriptedAgent::new("slow", Vec::new());
    slow.ages = Some(repo.clone());

    let report = run(&slow, &github, &repo, "write", input())
        .await
        .expect("write");

    assert_eq!(report.outcome, outcome("red"));
    assert!(
        report
            .output
            .contains("did not end before the step's time ran out")
    );
}

/// A retry past the Step's time takes what the agent handed back for the
/// open round before it ends the Step.
#[tokio::test]
async fn a_retry_past_its_time_reads_the_open_round_s_hand_back() {
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));
    let repo = FakeRepo::default();
    let input = input();
    let first = ScriptedAgent::new("done", vec![Err(FAILED)]);
    let _ = run(&first, &github, &repo, "write", input).await;
    // The agent handed back for the round's prompt after the attempt gave
    // up, and the Step's time ran out.
    let session = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(AgentName::try_from("primary".to_string()).expect("valid")),
        bot: Some(Uuid::now_v7()),
    };
    let writer = Role::try_from("writer".to_string()).expect("valid");
    repo.claim_session(input.key, session.clone(), &writer, &[Outcome::done()])
        .await
        .expect("claim");
    let round = &rounds(&repo, input).await[0];
    repo.begin_turn(
        input.key,
        session.session_id,
        round.check_action_id,
        2,
        &[outcome("passed"), outcome("failed")],
    )
    .await
    .expect("turn");
    let passed = HandBack {
        outcome: outcome("passed"),
        output: "web: exit 0".to_string(),
    };
    repo.hand_back(session.session_id, &passed)
        .await
        .expect("hand back");
    age(&repo, input.key);
    let retry = ScriptedAgent::new("done", Vec::new());

    let report = run(&retry, &github, &repo, "write", input)
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::done());
    assert!(report.output.contains("passed in round 1: web."));
    assert!(retry.turns().is_empty());
}

/// A red Step's report takes at most half of its output, so the agent's
/// work stays after it.
#[tokio::test]
async fn a_long_report_leaves_room_for_the_work() {
    let long: &'static str = Box::leak("x".repeat(MAX_OUTPUT_BYTES).into_boxed_str());
    let agent = ScriptedAgent::new(
        "done",
        vec![
            Ok(("failed", long)),
            Ok(("done", "Tried a fix")),
            Ok(("failed", long)),
        ],
    );
    let github = github(Some(CHECKS), Some(&["apps/web/index.ts"]));

    let report = run(&agent, &github, &FakeRepo::default(), "write", input())
        .await
        .expect("write");

    assert_eq!(report.outcome, outcome("red"));
    assert!(report.output.len() <= MAX_OUTPUT_BYTES);
    assert!(report.output.contains("Wrote the toggle"));
    assert!(report.output.contains("After round 1 failed: Tried a fix"));
}
