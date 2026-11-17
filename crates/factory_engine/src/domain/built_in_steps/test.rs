use std::sync::{Arc, LazyLock, Mutex};

use chrono::Utc;
use factory_records::domain::models::{
    EntryId, Gate, GateAnswer, Outcome, Repository, Role, RunId, Step, StepEnd, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{FindingKind, Lines, ReviewFinding, ReviewRound};
use macro_user_id::user_id::MacroUserIdStr;

use super::{BuiltInStepError, BuiltInSteps};

use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::flow::{Flow, StepName, StepUse};
use crate::domain::models::{AgentAnswer, AgentTask, AgentTurn, StepInput};
use crate::domain::ports::{AgentFailure, Retryable, StepAgent, StepExecutor};
use crate::domain::pull_request_steps::PullRequestSteps;

#[derive(Debug, thiserror::Error)]
#[error("fake agent failure")]
struct FakeError;

impl Retryable for FakeError {
    fn retryable(&self) -> bool {
        true
    }
}

impl AgentFailure for FakeError {}

/// Keeps every task it is given and ends each with `outcome`.
#[derive(Clone)]
struct FakeAgent {
    outcome: &'static str,
    tasks: Arc<Mutex<Vec<AgentTask>>>,
}

impl FakeAgent {
    fn ending_with(outcome: &'static str) -> Self {
        Self {
            outcome,
            tasks: Arc::default(),
        }
    }
}

impl StepAgent for FakeAgent {
    type Err = FakeError;

    async fn follow_up(&self, _turn: AgentTurn) -> Result<AgentAnswer, FakeError> {
        Err(FakeError)
    }

    async fn stop(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), FakeError> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), FakeError> {
        Ok(())
    }

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, FakeError> {
        self.tasks.lock().expect("tasks").push(task);
        Ok(AgentAnswer {
            outcome: Outcome::try_from(self.outcome.to_string()).expect("valid"),
            text: "The agent's work".to_string(),
        })
    }
}

fn pull_requests() -> PullRequestSteps<FakeGithub, FakeRepo> {
    PullRequestSteps::new(FakeGithub::default(), FakeRepo::default())
}

fn step(name: &str) -> StepName {
    StepName::try_from(name.to_string()).expect("valid")
}

static JULIA: LazyLock<MacroUserIdStr<'static>> = LazyLock::new(|| {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
});

static WRITER: LazyLock<Role> =
    LazyLock::new(|| Role::try_from("writer".to_string()).expect("valid"));

fn input<'a>(earlier: &'a [Step]) -> StepInput<'a> {
    StepInput {
        key: StepKey {
            run_id: RunId::generate(),
            seq: 2,
        },
        role: Some(&WRITER),
        owner: &JULIA,
        request: "Add a dark mode toggle",
        task_id: None,
        repository: None,
        earlier,
        review: None,
    }
}

#[test]
fn the_code_flow_s_steps_are_served() {
    let steps = BuiltInSteps::new(FakeAgent::ending_with("done"), pull_requests());

    for name in [
        "reply",
        "plan",
        "write",
        "review",
        "fix",
        "ci_fix",
        "publish",
        "ci",
        "ready_to_merge",
        "merge",
    ] {
        assert!(steps.serves(&step(name)).is_some(), "{name}");
    }
    assert!(steps.serves(&step("deploy")).is_none());
}

/// An agent Step goes to an Agent of the Role its entry names. A Run whose
/// Flow named no Roles, as one started before they were, goes on with the
/// Role its built-in Step implies.
#[tokio::test]
async fn an_agent_step_asks_for_the_role_its_entry_names_or_its_step_implies() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());
    let mut no_role = input(&[]);
    no_role.role = None;

    steps.run(&step("write"), input(&[])).await.expect("write");
    steps.run(&step("plan"), no_role).await.expect("plan");

    let roles: Vec<String> = agent
        .tasks
        .lock()
        .expect("tasks")
        .iter()
        .map(|task| task.role.to_string())
        .collect();
    assert_eq!(roles, ["writer", "planner"]);
    assert_eq!(steps.serves(&step("plan")), Some(StepUse::Agent));
    assert_eq!(steps.serves(&step("ci")), Some(StepUse::Work));
}

/// An agent Step of a Run that works on a repository works in its own
/// session's checkout of it, told the Run's branch and its base.
#[tokio::test]
async fn an_agent_step_works_in_the_run_s_repository_on_its_branch() {
    let agent = FakeAgent::ending_with("changes");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "trunk".to_string(),
    };
    let mut in_repository = input(&[]);
    in_repository.repository = Some(&repository);
    let branch = format!("factory/{}", in_repository.key.run_id);

    let report = steps
        .run(&step("review"), in_repository)
        .await
        .expect("review");

    assert_eq!(report.outcome.as_str(), "changes");
    let tasks = agent.tasks.lock().expect("tasks");
    assert_eq!(
        tasks[0].repo_url.as_deref(),
        Some("https://github.com/macro/app")
    );
    assert!(
        tasks[0]
            .prompt
            .contains(&format!("git push origin {branch}"))
    );
    assert!(tasks[0].prompt.contains("`origin/trunk`"));
    assert_eq!(
        tasks[0].outcomes,
        [
            Outcome::try_from("pass".to_string()).expect("valid"),
            Outcome::try_from("changes".to_string()).expect("valid"),
        ]
    );
}

/// A Step that only works on a repository fails for good in a Run with
/// none, before any agent is asked.
#[tokio::test]
async fn a_repository_step_of_a_run_with_no_repository_fails_for_good() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());

    let failed = steps.run(&step("fix"), input(&[])).await;

    let Err(error) = failed else {
        panic!("a fix needs a repository");
    };
    assert!(matches!(error, BuiltInStepError::NoRepository(_)));
    assert!(!error.retryable());
    assert!(agent.tasks.lock().expect("tasks").is_empty());
}

#[tokio::test]
async fn a_reply_is_done_and_answers_the_request() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());

    let report = steps.run(&step("reply"), input(&[])).await.expect("reply");

    assert_eq!(report.outcome, Outcome::done());
    assert!(report.output.contains("Add a dark mode toggle"));
    assert!(agent.tasks.lock().expect("tasks").is_empty());
}

#[tokio::test]
async fn a_plan_hands_the_request_to_the_agent_and_keeps_its_answer() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());

    let report = steps.run(&step("plan"), input(&[])).await.expect("plan");

    assert_eq!(report.outcome, Outcome::done());
    assert_eq!(report.output, "The agent's work");
    let tasks = agent.tasks.lock().expect("tasks");
    assert_eq!(tasks[0].outcomes, [Outcome::done()]);
    assert!(tasks[0].prompt.contains("Add a dark mode toggle"));
}

#[tokio::test]
async fn a_write_is_given_the_plan_an_earlier_step_produced() {
    let agent = FakeAgent::ending_with("unchanged");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());
    let earlier = [Step {
        seq: 1,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "Add a toggle to the settings page".to_string(),
            ended_at: Utc::now(),
        }),
        gate: None,
        checks: Vec::new(),
    }];

    let report = steps
        .run(&step("write"), input(&earlier))
        .await
        .expect("write");

    assert_eq!(report.outcome, Outcome::unchanged());
    let tasks = agent.tasks.lock().expect("tasks");
    assert!(
        tasks[0]
            .prompt
            .contains("Add a toggle to the settings page")
    );
}

#[tokio::test]
async fn an_outcome_the_step_does_not_have_fails_it() {
    let steps = BuiltInSteps::new(FakeAgent::ending_with("merged"), pull_requests());

    let refused = steps.run(&step("plan"), input(&[])).await;

    assert!(matches!(
        refused,
        Err(BuiltInStepError::UnexpectedOutcome { .. })
    ));
}

#[test]
fn the_plan_flow_runs_on_the_built_in_steps() {
    let steps = BuiltInSteps::new(FakeAgent::ending_with("done"), pull_requests());
    let definition = serde_json::from_str(include_str!("../../../flows/plan.json")).expect("JSON");

    let parsed = Flow::parse(definition, |name| steps.serves(name));

    assert!(parsed.is_ok(), "{parsed:?}");
}

#[tokio::test]
async fn a_plan_reads_what_a_person_answered_the_last_one() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());
    let revise = Outcome::try_from("revise".to_string()).expect("valid");
    let earlier = [Step {
        seq: 1,
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        started_at: Utc::now(),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome: Outcome::done(),
            output: "Rebuild the index".to_string(),
            ended_at: Utc::now(),
        }),
        gate: Some(Gate {
            question: "Approve the plan?".to_string(),
            answers: vec![Outcome::done(), revise.clone()],
            opened_at: Utc::now(),
            answer: Some(GateAnswer {
                answer: revise,
                note: Some("Keep the old index".to_string()),
                user_id: None,
                answered_at: Utc::now(),
            }),
        }),
        checks: Vec::new(),
    }];

    steps
        .run(&step("plan"), input(&earlier))
        .await
        .expect("plan");

    let tasks = agent.tasks.lock().expect("tasks");
    assert!(tasks[0].prompt.contains("revise"));
    assert!(tasks[0].prompt.contains("Keep the old index"));
}

/// An ended Step of entry `entry` at place `seq`, which produced `output`.
fn ended(seq: u16, entry: &str, visit: u16, output: &str) -> Step {
    Step {
        seq,
        entry: EntryId::try_from(entry.to_string()).expect("valid"),
        visit,
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
    }
}

/// A loop's earlier rounds are history the agent doesn't need, and a Run
/// that loops would otherwise grow its prompt without bound.
#[tokio::test]
async fn a_prompt_carries_the_latest_visit_of_each_entry_newest_first() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());
    let earlier = [
        ended(1, "plan", 1, "First plan"),
        ended(2, "review", 1, "First review"),
        ended(3, "plan", 2, "Second plan"),
        ended(4, "review", 2, "Second review"),
    ];

    steps
        .run(&step("write"), input(&earlier))
        .await
        .expect("write");

    let tasks = agent.tasks.lock().expect("tasks");
    let prompt = &tasks[0].prompt;
    assert!(!prompt.contains("First plan"));
    assert!(!prompt.contains("First review"));
    let review = prompt.find("Second review").expect("the latest review");
    let plan = prompt.find("Second plan").expect("the latest plan");
    assert!(review < plan, "newest first");
}

#[tokio::test]
async fn a_prompt_leaves_out_the_oldest_steps_past_its_budget() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());
    let earlier = [
        ended(1, "plan", 1, &"p".repeat(40 * 1024)),
        ended(2, "review", 1, &"r".repeat(40 * 1024)),
    ];

    steps
        .run(&step("write"), input(&earlier))
        .await
        .expect("write");

    let tasks = agent.tasks.lock().expect("tasks");
    let prompt = &tasks[0].prompt;
    assert!(prompt.contains(&"r".repeat(40 * 1024)));
    assert!(!prompt.contains(&"p".repeat(40 * 1024)));
    assert!(prompt.contains("1 earlier steps are left out"));
}

/// A plan may be as large as a Step's output allows, past what the prompt
/// has room for: the Step after it reads as much of it as fits, never none,
/// cut between two characters wherever the budget falls.
#[tokio::test]
async fn a_prompt_cuts_a_newest_step_past_its_budget_rather_than_leave_it_out() {
    // A 3-byte character, after pads of each length its width allows, so
    // the budget falls inside a character in at least one of them.
    for pad in 0..3 {
        let agent = FakeAgent::ending_with("done");
        let steps = BuiltInSteps::new(agent.clone(), pull_requests());
        let plan = format!("Plan:{} {}", "_".repeat(pad), "€".repeat(24 * 1024));
        let earlier = [
            ended(1, "request", 1, "Earlier"),
            ended(2, "plan", 1, &plan),
        ];

        steps
            .run(&step("write"), input(&earlier))
            .await
            .expect("write");

        let tasks = agent.tasks.lock().expect("tasks");
        let prompt = &tasks[0].prompt;
        assert!(prompt.contains("€€"), "pad {pad}");
        assert!(!prompt.contains(&plan), "pad {pad}");
        assert!(
            prompt.contains("The rest of this step is left out"),
            "pad {pad}"
        );
        assert!(prompt.contains("1 earlier steps are left out"), "pad {pad}");
        assert!(prompt.len() < plan.len(), "pad {pad}");
    }
}

/// A review round that starts at the Step `seq`, of two reviewers, which
/// reviews `head` from `diff_base`, with one Blocker and one follow-up once
/// it ended.
fn round(seq: u16, head: &str, diff_base: &str, fix_diff: bool) -> ReviewRound {
    let finding = |place, kind, path: &str| ReviewFinding {
        seq,
        place,
        role: Some(Role::try_from("tests-reviewer".to_string()).expect("valid")),
        path: path.to_string(),
        lines: Some(Lines { start: 3, end: 5 }),
        problem: format!("{path} skips the empty case"),
        why: "An empty list panics".to_string(),
        blocks_anywhere: false,
        kind: Some(kind),
        follow_up_task_id: None,
        follow_up_key: None,
    };
    ReviewRound {
        round: 2,
        seq,
        reviewers: 2,
        head_sha: Some(head.to_string()),
        diff_base: Some(diff_base.to_string()),
        fix_diff,
        failure: None,
        started_at: Utc::now(),
        end: None,
        findings: vec![
            finding(1, FindingKind::Blocker, "src/list.rs"),
            finding(2, FindingKind::FollowUp, "src/other.rs"),
        ],
        answered: Vec::new(),
    }
}

/// The fix is given the Blockers of the latest round, with where each is,
/// what is wrong and why, and not its follow-ups, inside a block marked as
/// the reviewers' findings.
#[tokio::test]
async fn a_fix_is_given_the_blockers_of_the_latest_round() {
    let agent = FakeAgent::ending_with("done");
    let repo = FakeRepo::default();
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(FakeGithub::default(), repo.clone()),
    );
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "trunk".to_string(),
    };
    let ended_round = round(2, "bbb", "aaa", true);
    let mut fixing = input(&[]);
    fixing.repository = Some(&repository);
    fixing.review = Some(&ended_round);
    let entry = EntryId::try_from("fix".to_string()).expect("valid");
    repo.start_step(fixing.key, &entry, 1).await.expect("start");

    steps.run(&step("fix"), fixing).await.ok();

    let prompt = &agent.tasks.lock().expect("tasks")[0].prompt;
    assert!(prompt.contains("## Blockers of review round 2"), "{prompt}");
    assert!(
        prompt.contains("1. `src/list.rs` lines 3-5: src/list.rs skips the empty case"),
        "{prompt}"
    );
    assert!(prompt.contains("Why: An empty list panics"), "{prompt}");
    assert!(!prompt.contains("src/other.rs"), "{prompt}");
    let open = prompt.find("<reviewer-findings>").expect("opened");
    let blocker = prompt.find("src/list.rs").expect("the blocker");
    let close = prompt.find("</reviewer-findings>").expect("closed");
    assert!(open < blocker && blocker < close, "{prompt}");
}

/// Each reviewer of a round says what it checked: a later Step reads them
/// all, not only the last to end.
#[tokio::test]
async fn a_prompt_carries_each_reviewer_s_latest_step() {
    let agent = FakeAgent::ending_with("done");
    let steps = BuiltInSteps::new(agent.clone(), pull_requests());
    let reviewer = |mut step: Step, role: &str| {
        step.role = Some(Role::try_from(role.to_string()).expect("valid"));
        step
    };
    let earlier = [
        reviewer(
            ended(3, "review", 1, "Correctness looks fine"),
            "correctness-reviewer",
        ),
        reviewer(
            ended(4, "review", 1, "Tests miss the empty list"),
            "tests-reviewer",
        ),
    ];

    steps
        .run(&step("write"), input(&earlier))
        .await
        .expect("write");

    let prompt = &agent.tasks.lock().expect("tasks")[0].prompt;
    assert!(prompt.contains("Correctness looks fine"));
    assert!(prompt.contains("Tests miss the empty list"));
}
