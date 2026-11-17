//! The Flow workflow and its activities on a running Temporal, with the Run
//! records in memory and an agent the test drives.
//!
//! Run with: `TEMPORAL_ADDRESS=http://127.0.0.1:7233 cargo test -p factory_engine -- --ignored`
//! against the local stack's Temporal dev server (`just run_local`). Each
//! test runs its own worker on a task queue of its own, so the stack's
//! factory worker never takes its work.

mod replay;
mod review_panel;

use factory_records::domain::standing_rules::{
    OrchestratorDefinition, RulesSource, RunRules, StandingRule,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::{
    NewRun, Outcome, Repository, Role, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use temporalio_client::errors::WorkflowGetResultError;
use temporalio_client::{
    Client, ClientOptions, ConnectionOptions, Url, WorkflowCancelOptions, WorkflowGetResultOptions,
    WorkflowStartOptions,
};
use temporalio_sdk::Runtime;
use tokio::sync::Notify;
use uuid::Uuid;

use super::worker_on;
use crate::domain::built_in_flows::BuiltInFlow;
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::fakes::{FakeGithub, FakeHoldTasks, FakeRepo, FakeWatches, ci};
use crate::domain::flow::Flow;
use crate::domain::gate::AnswerGate;
use crate::domain::main_hold::{MainWatch, MainWatchServiceImpl};
use crate::domain::models::{
    AgentAnswer, AgentTask, AgentTurn, PullRequestState, RequiredCheck, RequiredChecks, RunStart,
};
use crate::domain::ports::{
    AgentFailure, AnswerDelivery, MainWatches, Retryable, RunWorkflows, StepAgent,
};
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::review_rounds::{ChangedFile, ReviewRoundsImpl};
use crate::domain::service::StepServiceImpl;
use crate::outbound::temporal_main_watches::TemporalMainWatches;
use crate::outbound::temporal_run_workflows::TemporalRunWorkflows;
use crate::temporal::{FlowWorkflowDefinition, MainWatchWorkflowDefinition};

/// How long a test waits on Temporal before it fails.
const PATIENCE: Duration = Duration::from_secs(60);

/// What the agent does with a task.
#[derive(Clone, Copy)]
enum Work {
    /// Ends `done`.
    Ends,
    /// Fails in a way asking again would repeat.
    FailsForGood,
    /// Works until stopped.
    Works,
    /// Works until stopped the first given number of times it is asked,
    /// and ends `done` when asked again.
    WorksThenEnds(usize),
}

#[derive(Debug, thiserror::Error)]
#[error("the agent failed for good")]
struct AgentFailed;

impl Retryable for AgentFailed {
    fn retryable(&self) -> bool {
        false
    }
}

impl AgentFailure for AgentFailed {}

/// An agent that does `work`, says when a task reaches it, and counts the
/// times it was asked, stopped and let go, and the Steps it was asked for.
#[derive(Clone)]
struct Agent {
    work: Work,
    asked: Arc<Notify>,
    tasks: Arc<Mutex<Vec<StepKey>>>,
    stops: Arc<AtomicUsize>,
    stopped_at: Arc<Mutex<Option<tokio::time::Instant>>>,
    releases: Arc<AtomicUsize>,
}

impl Agent {
    fn doing(work: Work) -> Self {
        Self {
            work,
            asked: Arc::default(),
            tasks: Arc::default(),
            stops: Arc::default(),
            stopped_at: Arc::default(),
            releases: Arc::default(),
        }
    }
}

impl StepAgent for Agent {
    type Err = AgentFailed;

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, AgentFailed> {
        let asked = {
            let mut tasks = self.tasks.lock().expect("tasks");
            tasks.push(task.key);
            tasks.len()
        };
        self.asked.notify_one();
        let ends = AgentAnswer {
            outcome: Outcome::done(),
            text: "The plan".to_string(),
        };
        match self.work {
            Work::Ends => Ok(ends),
            Work::FailsForGood => Err(AgentFailed),
            Work::Works => std::future::pending().await,
            Work::WorksThenEnds(works) if asked <= works => std::future::pending().await,
            Work::WorksThenEnds(_) => Ok(ends),
        }
    }

    /// The Runs here work on no repository, so no Step runs checks.
    async fn follow_up(&self, _turn: AgentTurn) -> Result<AgentAnswer, AgentFailed> {
        Err(AgentFailed)
    }

    async fn run_check(
        &self,
        _command: crate::domain::models::AgentCommand,
    ) -> Result<Option<agent_session::domain::session_command::SessionCommandOutput>, AgentFailed>
    {
        Ok(None)
    }

    async fn stop(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), AgentFailed> {
        self.stops.fetch_add(1, Ordering::SeqCst);
        self.stopped_at
            .lock()
            .expect("stopped at")
            .get_or_insert_with(tokio::time::Instant::now);
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), AgentFailed> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The built-in Steps over `agent`, with no pull request on GitHub.
fn steps_of<A: StepAgent + Clone>(
    agent: &A,
    repo: &FakeRepo,
) -> BuiltInSteps<A, FakeGithub, FakeRepo, FakeWatches> {
    BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(FakeGithub::default(), repo.clone(), FakeWatches::default()),
    )
}

/// The watches of base branches, over `repo`, with no base on GitHub.
fn watches(repo: &FakeRepo) -> MainWatchServiceImpl<FakeRepo, FakeGithub, FakeHoldTasks> {
    MainWatchServiceImpl::new(
        repo.clone(),
        FakeGithub::default(),
        FakeHoldTasks::default(),
    )
}

/// A client of the Temporal at `TEMPORAL_ADDRESS`, the stack's by default.
async fn client() -> Client {
    let address = macro_env_var::maybe_read_env("TEMPORAL_ADDRESS")
        .unwrap_or_else(|| "http://127.0.0.1:7233".to_owned());
    Client::connect(
        ConnectionOptions::new(Url::parse(&address).expect("valid Temporal address"))
            .identity("factory_engine tests".to_string())
            .build(),
        ClientOptions::new("default").build(),
    )
    .await
    .expect("Temporal is up")
}

/// A Run of Julia's whose one entry `plan` ends it `success` when done.
async fn plan_run(repo: &FakeRepo) -> RunStart {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid");
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Add a dark mode toggle",
            user_id: &julia,
            task_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let flow = Flow::parse(
        json!({
            "entries": [{
                "id": "plan",
                "step": "plan",
                "role": "planner",
                "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
            }]
        }),
        |_| Some(crate::domain::flow::StepUse::Work),
    )
    .expect("valid flow");
    RunStart {
        run_id: run.id,
        flow,
    }
}

/// Walk a plan Run with `agent` on a worker of the test's own, running
/// `drive` on the started workflow, and return how the workflow ended and
/// the Run's records.
async fn walk<F>(
    agent: &Agent,
    drive: impl FnOnce(Client, RunId) -> F,
) -> (Result<RunOutcome, WorkflowGetResultError>, FakeRepo, RunId)
where
    F: Future<Output = ()>,
{
    walk_with(agent, None, drive).await
}

/// [`walk`] a Run that ended `outcome` before its workflow started.
async fn walk_after<F>(
    agent: &Agent,
    outcome: RunOutcome,
    drive: impl FnOnce(Client, RunId) -> F,
) -> (Result<RunOutcome, WorkflowGetResultError>, FakeRepo, RunId)
where
    F: Future<Output = ()>,
{
    walk_with(agent, Some(outcome), drive).await
}

async fn walk_with<F>(
    agent: &Agent,
    ended_before: Option<RunOutcome>,
    drive: impl FnOnce(Client, RunId) -> F,
) -> (Result<RunOutcome, WorkflowGetResultError>, FakeRepo, RunId)
where
    F: Future<Output = ()>,
{
    let client = client().await;
    let repo = FakeRepo::default();
    let start = plan_run(&repo).await;
    let run_id = start.run_id;
    if let Some(outcome) = ended_before {
        repo.end_run(run_id, outcome).await.expect("end");
    }
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        StepServiceImpl::new(repo.clone(), steps_of(agent, &repo)),
        ReviewRoundsImpl::new(repo.clone(), FakeGithub::default(), repo.clone()),
        watches(&repo),
    )
    .expect("worker");

    let done = Arc::new(Notify::new());
    let walked = async {
        let handle = client
            .start_workflow(
                FlowWorkflowDefinition,
                start,
                WorkflowStartOptions::new(task_queue.clone(), run_id.to_string()).build(),
            )
            .await
            .expect("start");
        drive(client.clone(), run_id).await;
        let ended = tokio::time::timeout(
            PATIENCE,
            handle.get_result(WorkflowGetResultOptions::default()),
        )
        .await
        .expect("the workflow ends");
        done.notify_one();
        ended
    };
    let (worked, ended) = tokio::join!(worker.run_until(done.notified()), walked);
    worked.expect("the worker ran");
    (ended, repo, run_id)
}

/// Wait until a task reached `agent`.
async fn asked(agent: &Agent) {
    tokio::time::timeout(PATIENCE, agent.asked.notified())
        .await
        .expect("the step reached the agent");
}

/// The activity cancelled with the Run can't tell why it was: the workflow
/// stops the agent, once, and the Run and its Step end cancelled. The cancel
/// reaches the working Step within a few seconds, not on the heartbeat
/// timeout's schedule.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn cancelling_a_run_stops_its_working_step_s_agent_once() {
    let agent = Agent::doing(Work::Works);

    let cancelled_at = Arc::new(Mutex::new(None));
    let (ended, repo, run_id) = walk(&agent, |client, run_id| {
        let agent = agent.clone();
        let cancelled_at = cancelled_at.clone();
        async move {
            // The Step's work just started, and with it its first heartbeat:
            // the next one is held back as long as the worker allows.
            asked(&agent).await;
            *cancelled_at.lock().expect("cancelled at") = Some(tokio::time::Instant::now());
            client
                .get_workflow_handle::<FlowWorkflowDefinition>(run_id.to_string())
                .cancel(WorkflowCancelOptions::builder().build())
                .await
                .expect("cancel");
        }
    })
    .await;

    assert!(matches!(
        ended,
        Err(WorkflowGetResultError::Cancelled { .. })
    ));
    assert_eq!(agent.stops.load(Ordering::SeqCst), 1);
    assert_eq!(agent.releases.load(Ordering::SeqCst), 0);
    let cancelled_at = cancelled_at
        .lock()
        .expect("cancelled at")
        .expect("cancelled");
    let stopped_at = agent
        .stopped_at
        .lock()
        .expect("stopped at")
        .expect("stopped");
    let latency = stopped_at - cancelled_at;
    assert!(latency < Duration::from_secs(10), "{latency:?}");
    let run = repo.get_run(run_id).await.expect("read").expect("kept");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Cancelled));
    let steps = repo.list_steps(run_id).await.expect("read");
    assert_eq!(
        steps[0].end.as_ref().map(|end| end.outcome.as_str()),
        Some("cancelled")
    );
}

/// A workflow that starts after its Run was cancelled, as one whose start
/// raced the cancel, takes no Step and ends cancelled, never failed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_workflow_whose_run_was_cancelled_before_it_started_ends_cancelled() {
    let agent = Agent::doing(Work::Ends);

    let (ended, repo, run_id) = walk_after(&agent, RunOutcome::Cancelled, |_, _| async {}).await;

    assert!(
        matches!(ended, Err(WorkflowGetResultError::Cancelled { .. })),
        "{ended:?}"
    );
    assert!(agent.tasks.lock().expect("tasks").is_empty());
    assert!(repo.list_steps(run_id).await.expect("read").is_empty());
}

/// A workflow that starts after the sweep ended its Run failed ends as the
/// Run did, not as a cancelled one.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_workflow_whose_run_ended_before_it_started_ends_as_the_run_did() {
    let agent = Agent::doing(Work::Ends);

    let (ended, repo, run_id) = walk_after(&agent, RunOutcome::Failed, |_, _| async {}).await;

    assert!(matches!(ended, Ok(RunOutcome::Failed)), "{ended:?}");
    assert!(agent.tasks.lock().expect("tasks").is_empty());
    assert!(repo.list_steps(run_id).await.expect("read").is_empty());
}

/// A Step that fails for good has its agent stopped, so nothing after it
/// runs beside it, and ends `failed` with nothing internal in its output.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_step_that_fails_for_good_has_its_agent_stopped() {
    let agent = Agent::doing(Work::FailsForGood);

    let (ended, repo, run_id) = walk(&agent, |_, _| async {}).await;

    assert!(matches!(ended, Ok(RunOutcome::Failed)), "{ended:?}");
    assert_eq!(agent.stops.load(Ordering::SeqCst), 1);
    let steps = repo.list_steps(run_id).await.expect("read");
    let end = steps[0].end.as_ref().expect("ended");
    assert_eq!(end.outcome, Outcome::failed());
    assert!(!end.output.contains("agent failed"));
}

/// A Step that ended has its agent let go once its end is recorded, and is
/// never stopped.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_step_that_ends_has_its_agent_let_go() {
    let agent = Agent::doing(Work::Ends);

    let (ended, repo, run_id) = walk(&agent, |_, _| async {}).await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    assert_eq!(agent.releases.load(Ordering::SeqCst), 1);
    assert_eq!(agent.stops.load(Ordering::SeqCst), 0);
    let steps = repo.list_steps(run_id).await.expect("read");
    assert_eq!(
        steps[0].end.as_ref().map(|end| end.output.as_str()),
        Some("The plan")
    );
}

/// How many times the tests shut a Step's worker down under it: more than
/// a count of attempts would allow.
const SHUTDOWNS: usize = 3;

/// A worker shut down while a Step works, as on a deploy, gives the Step a
/// few seconds, then drops its work; Temporal hands the Step to the next
/// worker at once, not after the heartbeat timeout, and the agent goes on
/// with the same Step, never stopped. However many deploys come, they never
/// use the Step up.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_step_whose_worker_shuts_down_goes_on_at_once_on_the_next() {
    let agent = Agent::doing(Work::WorksThenEnds(SHUTDOWNS));
    let client = client().await;
    let repo = FakeRepo::default();
    let start = plan_run(&repo).await;
    let run_id = start.run_id;
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let steps = || StepServiceImpl::new(repo.clone(), steps_of(&agent, &repo));
    let reviews = || ReviewRoundsImpl::new(repo.clone(), FakeGithub::default(), repo.clone());
    let handle = client
        .start_workflow(
            FlowWorkflowDefinition,
            start,
            WorkflowStartOptions::new(task_queue.clone(), run_id.to_string()).build(),
        )
        .await
        .expect("start");

    // Each worker shuts down once the Step reaches the agent; the next one
    // gets it within a few seconds.
    let mut restarted = None;
    for _ in 0..SHUTDOWNS {
        let worker = worker_on(
            &task_queue,
            &runtime,
            client.clone(),
            steps(),
            reviews(),
            watches(&repo),
        )
        .expect("worker");
        let reached = async {
            asked(&agent).await;
            if let Some(restarted) = restarted {
                let waited = tokio::time::Instant::elapsed(&restarted);
                assert!(waited < Duration::from_secs(30), "{waited:?}");
            }
        };
        tokio::time::timeout(PATIENCE, worker.run_until(reached))
            .await
            .expect("the worker shuts down")
            .expect("the worker ran");
        restarted = Some(tokio::time::Instant::now());
    }

    let last = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        steps(),
        reviews(),
        watches(&repo),
    )
    .expect("worker");
    let done = Notify::new();
    let walked = async {
        let ended = tokio::time::timeout(
            PATIENCE,
            handle.get_result(WorkflowGetResultOptions::default()),
        )
        .await
        .expect("the workflow ends");
        done.notify_one();
        ended
    };
    let (worked, ended) = tokio::join!(last.run_until(done.notified()), walked);
    worked.expect("the last worker ran");

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let tasks = agent.tasks.lock().expect("tasks").clone();
    assert_eq!(tasks.len(), SHUTDOWNS + 1);
    assert!(tasks.iter().all(|task| *task == tasks[0]));
    assert_eq!(agent.stops.load(Ordering::SeqCst), 0);
    assert_eq!(agent.releases.load(Ordering::SeqCst), 1);
}

/// The agent of a `code` Run: it ends each Step it is asked for `done`, but
/// a reviewer's with the next of `reviews` (`pass` once they run out:
/// `changes` hands back a Blocker in the change, `outside` one in a file it
/// does not touch), and a CI fix by pushing a commit whose checks pass. With
/// `together`, the reviewers of a round answer only once all of them were
/// asked. It says when it is asked for a Step, and keeps the Steps it was
/// asked for, by name.
#[derive(Clone)]
struct CodeAgent {
    github: FakeGithub,
    reviews: Arc<Mutex<Vec<&'static str>>>,
    together: Option<Arc<tokio::sync::Barrier>>,
    steps: Arc<Mutex<Vec<String>>>,
    asked: Arc<Notify>,
}

/// What a reviewer hands back for `review`, one of [`CodeAgent`]'s
/// `reviews`: its outcome and its text, with its Blockers' json block.
fn reviewed(review: &str) -> (&'static str, String) {
    let blocker = |file: &str| {
        format!(
            "Checked it.\n```json\n{{\"blockers\": [{{\"file\": \"{file}\", \"startLine\": 2, \
             \"problem\": \"Collapses one dash too many\", \"why\": \"Slugs lose a word\"}}]}}\n```"
        )
    };
    match review {
        "changes" => ("changes", blocker("src/slug.rs")),
        "outside" => ("changes", blocker("README.md")),
        _ => (
            "pass",
            "Checked it.\n```json\n{\"blockers\": []}\n```".to_string(),
        ),
    }
}

/// The commit a CI fix pushes.
const FIXED_HEAD: &str = "2222222222222222222222222222222222222222";

impl CodeAgent {
    fn new(github: &FakeGithub) -> Self {
        Self {
            github: github.clone(),
            reviews: Arc::default(),
            together: None,
            steps: Arc::default(),
            asked: Arc::default(),
        }
    }

    fn steps(&self) -> Vec<String> {
        self.steps.lock().expect("steps").clone()
    }
}

impl StepAgent for CodeAgent {
    type Err = AgentFailed;

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, AgentFailed> {
        let step = task.step.as_str().to_string();
        self.steps.lock().expect("steps").push(step.clone());
        self.asked.notify_one();
        if step == "review" {
            let review = {
                let mut reviews = self.reviews.lock().expect("reviews");
                if reviews.is_empty() {
                    "pass"
                } else {
                    reviews.remove(0)
                }
            };
            if let Some(together) = &self.together {
                tokio::time::timeout(PATIENCE, together.wait())
                    .await
                    .expect("the round's reviewers work at once");
            }
            let (outcome, text) = reviewed(review);
            return Ok(AgentAnswer {
                outcome: Outcome::try_from(outcome.to_string()).expect("valid"),
                text,
            });
        }
        let outcome = match step.as_str() {
            "ci_fix" => {
                let mut github = self.github.state();
                if let Some(pull_request) = github.pull_request.as_mut() {
                    pull_request.head_sha = FIXED_HEAD.to_string();
                }
                github.checks = [ci(Some("success"))].into();
                "done"
            }
            _ => "done",
        };
        Ok(AgentAnswer {
            outcome: Outcome::try_from(outcome.to_string()).expect("valid"),
            text: format!("The {step}"),
        })
    }

    /// Its repository declares no checks, so no Step asks.
    async fn follow_up(&self, _turn: AgentTurn) -> Result<AgentAnswer, AgentFailed> {
        Err(AgentFailed)
    }

    async fn run_check(
        &self,
        _command: crate::domain::models::AgentCommand,
    ) -> Result<Option<agent_session::domain::session_command::SessionCommandOutput>, AgentFailed>
    {
        Ok(None)
    }

    async fn stop(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), AgentFailed> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), AgentFailed> {
        Ok(())
    }
}

/// What a test driving a `code` Run is handed: Temporal, the Run, and its
/// records.
struct CodeRun {
    client: Client,
    run_id: RunId,
    repo: FakeRepo,
}

impl CodeRun {
    /// Wait for the Run's next Gate to open, and return its Step's place.
    async fn next_gate(&self) -> u16 {
        tokio::time::timeout(PATIENCE, self.repo.gate_opened.notified())
            .await
            .expect("a gate opens");
        self.repo
            .list_steps(self.run_id)
            .await
            .expect("read")
            .iter()
            .filter(|step| step.gate.is_some())
            .map(|step| step.seq)
            .max()
            .expect("an open gate")
    }

    /// Give the Gate of the Step `seq` the answer `answer`, as Julia. The
    /// Gate is recorded just before the workflow asks its Standing Rules
    /// and waits on it, and an answer that lands in between is refused, as
    /// the API would refuse it: the answer is sent again for a few seconds.
    async fn answer(&self, seq: u16, answer: &str) {
        assert!(self.try_answer(seq, answer).await, "the answer was taken");
    }

    /// [`Self::answer`], or false once a Standing Rule answered the Gate.
    async fn try_answer(&self, seq: u16, answer: &str) -> bool {
        let workflows = TemporalRunWorkflows::new(self.client.clone());
        let answer = AnswerGate {
            seq,
            answer: Outcome::try_from(answer.to_string()).expect("valid"),
            note: None,
            user_id: julia(),
            approved_control_files: None,
        };
        for _ in 0..100 {
            let delivery = workflows
                .answer(self.run_id, &answer)
                .await
                .expect("answer");
            if delivery == AnswerDelivery::Taken {
                return true;
            }
            if self.ruled(seq).await {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    /// Whether a Standing Rule answered the Gate of the Step `seq`.
    async fn ruled(&self, seq: u16) -> bool {
        self.repo
            .list_steps(self.run_id)
            .await
            .expect("read")
            .into_iter()
            .find(|step| step.seq == seq)
            .and_then(|step| step.gate?.answer)
            .is_some_and(|answer| answer.rule.is_some())
    }

    /// Wait for the next Gate a person is asked, and give it `answer`;
    /// Gates a Standing Rule answers are passed by.
    async fn answer_next_gate(&self, answer: &str) {
        loop {
            let seq = self.next_gate().await;
            if self.try_answer(seq, answer).await {
                return;
            }
        }
    }

    async fn cancel(&self) {
        self.client
            .get_workflow_handle::<FlowWorkflowDefinition>(self.run_id.to_string())
            .cancel(WorkflowCancelOptions::builder().build())
            .await
            .expect("cancel");
    }
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

/// Walk a Run of the built-in `code` Flow on a repository, with `agent` and
/// `github`, running `drive` on the started workflow; return how it ended.
async fn walk_code<F>(
    agent: &CodeAgent,
    github: &FakeGithub,
    drive: impl FnOnce(CodeRun) -> F,
) -> Result<RunOutcome, WorkflowGetResultError>
where
    F: Future<Output = ()>,
{
    walk_code_with(
        agent,
        github,
        &FakeRepo::default(),
        RunRules::DEFAULT,
        None,
        None,
        drive,
    )
    .await
}

/// [`walk_code`] a Run kept in `repo` with the Standing Rules `rules`,
/// its review panel bounded to `rounds` rounds when given.
async fn walk_code_with<F>(
    agent: &CodeAgent,
    github: &FakeGithub,
    repo: &FakeRepo,
    rules: RunRules,
    rounds: Option<std::num::NonZeroU8>,
    flow: Option<Flow>,
    drive: impl FnOnce(CodeRun) -> F,
) -> Result<RunOutcome, WorkflowGetResultError>
where
    F: Future<Output = ()>,
{
    let client = client().await;
    let repo = repo.clone();
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "code",
            request: "Collapse repeated dashes in slugs",
            user_id: &julia(),
            task_id: None,
            repository: Some(&repository),
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: rules,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let mut flow = flow.unwrap_or_else(|| {
        BuiltInFlow::named("code")
            .expect("built in")
            .read(|_| Some(crate::domain::flow::StepUse::Work))
            .expect("valid flow")
    });
    if let Some(rounds) = rounds {
        flow.bound_review_rounds(rounds);
    }
    // The Run's branch is at the pull request's head, and its change is to
    // `src/slug.rs`.
    github.state().branch_heads = [Some(FakeGithub::HEAD.to_string())].into();
    github.state().diffs.insert(
        ("main".to_string(), FakeGithub::HEAD.to_string()),
        vec![ChangedFile::from_patch(
            "src/slug.rs".to_string(),
            None,
            Some("@@ -1,3 +1,6 @@"),
        )],
    );
    let start = RunStart {
        run_id: run.id,
        flow,
    };
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    );
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        StepServiceImpl::new(repo.clone(), steps),
        ReviewRoundsImpl::new(repo.clone(), github.clone(), repo.clone()),
        watches(&repo),
    )
    .expect("worker");

    let done = Arc::new(Notify::new());
    let walked = async {
        let handle = client
            .start_workflow(
                FlowWorkflowDefinition,
                start,
                WorkflowStartOptions::new(task_queue.clone(), run.id.to_string()).build(),
            )
            .await
            .expect("start");
        drive(CodeRun {
            client: client.clone(),
            run_id: run.id,
            repo: repo.clone(),
        })
        .await;
        let ended = tokio::time::timeout(
            PATIENCE,
            handle.get_result(WorkflowGetResultOptions::default()),
        )
        .await
        .expect("the workflow ends");
        done.notify_one();
        ended
    };
    let (worked, ended) = tokio::join!(worker.run_until(done.notified()), walked);
    worked.expect("the worker ran");
    ended
}

/// A `code` Run opens one pull request and merges it only once a person
/// approves the commit whose checks passed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_code_run_merges_its_pull_request_once_a_person_approves() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);

    let ended = walk_code(&agent, &github, |run| {
        let github = github.clone();
        async move {
            run.answer_next_gate("approve").await;
            let merge_question = run.next_gate().await;
            // The person is asked; nothing merged yet.
            assert!(github.state().merges.is_empty());
            run.answer(merge_question, "approve").await;
        }
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    assert_eq!(
        agent.steps(),
        ["plan", "write", "review", "review", "review"]
    );
    let github = github.state();
    assert_eq!(github.opened, 1);
    assert_eq!(github.merges, [FakeGithub::HEAD]);
    assert_eq!(
        github
            .pull_request
            .as_ref()
            .map(|pull_request| pull_request.state),
        Some(PullRequestState::Merged)
    );
}

/// CI that fails is fixed by an agent, reviewed again, and merges green, on
/// the commit the fix pushed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn red_ci_is_fixed_and_the_fixed_commit_merges() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("failure"))].into();
    let agent = CodeAgent::new(&github);

    let ended = walk_code(&agent, &github, |run| async move {
        run.answer_next_gate("approve").await;
        run.answer_next_gate("approve").await;
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    assert_eq!(
        agent.steps(),
        [
            "plan", "write", "review", "review", "review", "ci_fix", "review", "review", "review"
        ]
    );
    let github = github.state();
    assert_eq!(github.opened, 1);
    assert_eq!(github.merges, [FIXED_HEAD]);
}

/// A person who sends the merge back has the change fixed and is asked
/// again; cancelling then ends the Run with nothing merged.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn revising_or_cancelling_at_the_merge_question_merges_nothing() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);

    let ended = walk_code(&agent, &github, |run| async move {
        run.answer_next_gate("approve").await;
        run.answer_next_gate("revise").await;
        run.next_gate().await;
        run.cancel().await;
    })
    .await;

    assert!(
        matches!(ended, Err(WorkflowGetResultError::Cancelled { .. })),
        "{ended:?}"
    );
    assert_eq!(
        agent.steps(),
        [
            "plan", "write", "review", "review", "review", "fix", "review", "review", "review"
        ]
    );
    let github = github.state();
    assert!(github.merges.is_empty());
    assert_eq!(github.opened, 1);
}

/// Review rounds that keep finding Blockers stop after three: the Run ends
/// for a person, with no pull request opened.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_review_that_keeps_finding_blockers_ends_the_run_for_a_person() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    // One reviewer of each round of three finds a Blocker.
    *agent.reviews.lock().expect("reviews") = [["changes", "pass", "pass"]; 3].concat();
    let repo = FakeRepo::default();

    // The real Flow's three rounds: the default rules fix the first two
    // rounds' Blockers, and the third round's end the Run.
    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        RunRules::DEFAULT,
        None,
        None,
        |run| async move {
            run.answer_next_gate("approve").await;
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Manual)), "{ended:?}");
    let steps = agent.steps();
    assert_eq!(steps.iter().filter(|step| *step == "review").count(), 9);
    // The last round's Blockers end the Run: no fix runs after it.
    assert_eq!(steps.iter().filter(|step| *step == "fix").count(), 2);
    let ruled: Vec<StandingRule> = repo
        .rule_answers
        .lock()
        .expect("answers")
        .iter()
        .map(|(_, _, rule)| *rule)
        .collect();
    assert_eq!(
        ruled,
        [StandingRule::FixBlockers, StandingRule::FixBlockers]
    );
    let github = github.state();
    assert_eq!(github.opened, 0);
    assert!(github.merges.is_empty());
}

/// A Run whose Standing Rules decide every Gate goes from its plan to a
/// merged pull request with no person asked: each Gate is answered by its
/// rule, and the answer names it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_run_whose_rules_decide_every_gate_merges_with_no_person_asked() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    *agent.reviews.lock().expect("reviews") = [["changes", "pass", "pass"]; 1].concat();
    let repo = FakeRepo::default();
    let rules = RunRules {
        definition: OrchestratorDefinition {
            plan: true,
            blockers: true,
            merge: true,
        },
        source: RulesSource::Team,
        project_id: None,
    };

    let ended = walk_code_with(&agent, &github, &repo, rules, None, None, |_| async {}).await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    assert_eq!(github.state().merges, [FakeGithub::HEAD]);
    let ruled: Vec<StandingRule> = repo
        .rule_answers
        .lock()
        .expect("answers")
        .iter()
        .map(|(_, _, rule)| *rule)
        .collect();
    assert_eq!(
        ruled,
        [
            StandingRule::ApprovePlan,
            StandingRule::FixBlockers,
            StandingRule::MergeWhenGreen
        ]
    );
    assert!(repo.approvals.lock().expect("approvals").is_empty());
}

/// A Gate whose Flow names no kind is never answered by a rule, even on a
/// plan entry of a Run whose rules decide every kind: a person answers it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_gate_of_no_kind_waits_for_a_person_whatever_the_rules() {
    let github = FakeGithub::default();
    let agent = CodeAgent::new(&github);
    let repo = FakeRepo::default();
    let rules = RunRules {
        definition: OrchestratorDefinition {
            plan: true,
            blockers: true,
            merge: true,
        },
        source: RulesSource::Team,
        project_id: None,
    };
    let flow = Flow::parse(
        json!({
            "entries": [{
                "id": "plan",
                "step": "plan",
                "role": "planner",
                "gate": { "question": "Approve?", "answers": ["approve", "revise"] },
                "on": {
                    "approve": [{ "to": { "kind": "end", "outcome": "success" } }],
                    "revise": [{ "to": { "kind": "end", "outcome": "manual" } }]
                }
            }]
        }),
        |_| Some(crate::domain::flow::StepUse::Work),
    )
    .expect("valid flow");

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        rules,
        None,
        Some(flow),
        |run| async move {
            run.answer_next_gate("revise").await;
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Manual)), "{ended:?}");
    assert!(repo.rule_answers.lock().expect("answers").is_empty());
}

/// Review Blockers go to a fix on their own twice in a row; when the third
/// round still finds some, a person is asked, and stopping ends the Run
/// for them.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn blockers_after_two_rule_fixes_wait_for_a_person() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    *agent.reviews.lock().expect("reviews") = [["changes", "pass", "pass"]; 5].concat();
    let repo = FakeRepo::default();

    let ended = walk_code_with(
        &agent,
        &github,
        &repo,
        RunRules::DEFAULT,
        std::num::NonZeroU8::new(5),
        None,
        |run| async move {
            run.answer_next_gate("approve").await;
            // The two fixes need no one; the third round's Blockers do.
            run.answer_next_gate("stop").await;
        },
    )
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Manual)), "{ended:?}");
    let steps = agent.steps();
    assert_eq!(steps.iter().filter(|step| *step == "review").count(), 9);
    assert_eq!(steps.iter().filter(|step| *step == "fix").count(), 2);
    assert_eq!(repo.rule_answers.lock().expect("answers").len(), 2);
    assert!(github.state().merges.is_empty());
}

/// Cancelling a Run while CI waits on checks that never finish stops the
/// wait within seconds and merges nothing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn cancelling_while_ci_waits_ends_the_run_with_nothing_merged() {
    let github = FakeGithub::default();
    github.state().checks = [ci(None)].into();
    let agent = CodeAgent::new(&github);

    let ended = walk_code(&agent, &github, |run| {
        let github = github.clone();
        async move {
            run.answer_next_gate("approve").await;
            // Publish looks the pull request up twice; CI's first look is
            // the third.
            while github.state().reads < 3 {
                tokio::time::timeout(PATIENCE, github.read.notified())
                    .await
                    .expect("the pull request is read");
            }
            run.cancel().await;
        }
    })
    .await;

    assert!(
        matches!(ended, Err(WorkflowGetResultError::Cancelled { .. })),
        "{ended:?}"
    );
    assert!(github.state().merges.is_empty());
}

/// A head that moves after the person agreed to merge is not merged: the
/// Run goes back to CI, asks again, and merges the new head once agreed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_head_that_moves_before_the_merge_goes_back_to_ci() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);

    let ended = walk_code(&agent, &github, |run| {
        let github = github.clone();
        async move {
            run.answer_next_gate("approve").await;
            let merge_question = run.next_gate().await;
            // Someone pushes after the person was asked.
            if let Some(pull_request) = github.state().pull_request.as_mut() {
                pull_request.head_sha = FIXED_HEAD.to_string();
            }
            run.answer(merge_question, "approve").await;
            run.answer_next_gate("approve").await;
        }
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let github = github.state();
    assert_eq!(github.merges, [FakeGithub::HEAD, FIXED_HEAD]);
    assert_eq!(github.opened, 1);
}

/// A watch over `repo` and `github`, on a task queue of its own, and the
/// base of a new Run of Julia's it watches.
async fn watching(repo: &FakeRepo) -> (MainWatch, Repository) {
    let julia = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid");
    let repository = Repository {
        url: format!("https://github.com/macro/app-{}", Uuid::now_v7().simple()),
        base_branch: "main".to_string(),
    };
    let run_id = RunId::generate();
    repo.insert_run(NewRun {
        id: run_id,
        flow: "code",
        request: "Add dark mode",
        user_id: &julia,
        task_id: None,
        repository: Some(&repository),
        created_in_person: false,
        control_file_patterns: &[],
        control_files_team_id: None,
        standing_rules: RunRules::DEFAULT,
    })
    .await
    .expect("insert");
    (MainWatch::of(run_id, &repository), repository)
}

/// Run a worker for the watch over `repo`, `github` and `tasks` until
/// `drive` returns, and return what it returned.
async fn with_watch_worker<T>(
    repo: &FakeRepo,
    github: &FakeGithub,
    tasks: &FakeHoldTasks,
    drive: impl AsyncFnOnce(Client, TemporalMainWatches) -> T,
) -> T {
    let client = client().await;
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        StepServiceImpl::new(repo.clone(), steps_of(&Agent::doing(Work::Works), repo)),
        ReviewRoundsImpl::new(repo.clone(), github.clone(), repo.clone()),
        MainWatchServiceImpl::new(repo.clone(), github.clone(), tasks.clone()),
    )
    .expect("worker");
    let watches = TemporalMainWatches::on(client.clone(), &task_queue);
    let done = Notify::new();
    let driven = async {
        let driven = drive(client, watches).await;
        done.notify_one();
        driven
    };
    let (worked, driven) = tokio::join!(worker.run_until(done.notified()), driven);
    worked.expect("the worker ran");
    driven
}

/// The watch's end, once it ends.
async fn watch_end(client: &Client, watch: &MainWatch) {
    tokio::time::timeout(
        PATIENCE,
        client
            .get_workflow_handle::<MainWatchWorkflowDefinition>(watch.workflow_id())
            .get_result(WorkflowGetResultOptions::default()),
    )
    .await
    .expect("the watch ends")
    .expect("the watch ended well");
}

/// The watch of a red base holds it and files its task once, and a second
/// merge's signal reaches the watch that runs. Held, it looks past the
/// looks one run takes, continuing as new, and ends once the base's checks
/// pass, clearing the hold.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_base_s_watch_holds_it_while_red_and_ends_once_green() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    github.state().required = Some(RequiredChecks {
        checks: vec![RequiredCheck {
            name: "test".to_string(),
            app_id: None,
        }],
        complete: true,
    });
    github.state().base = Some((
        "2222222222222222222222222222222222222222".to_string(),
        ci(Some("failure")),
    ));
    let tasks = FakeHoldTasks::default();
    let (watch, repository) = watching(&repo).await;

    with_watch_worker(&repo, &github, &tasks, async |client, watches| {
        watches.watch(&watch).await.expect("start");
        tokio::time::timeout(PATIENCE, tasks.filing.notified())
            .await
            .expect("the task is filed");
        watches.watch(&watch).await.expect("signal");
        // Past the three looks one run takes in the tests.
        for _ in 0..5 {
            tokio::time::timeout(PATIENCE, github.base_read.notified())
                .await
                .expect("the watch looks again");
        }
        assert!(
            repo.main_hold(&repository)
                .await
                .expect("read")
                .is_some_and(|hold| hold.task_id.is_some())
        );

        github.state().base = Some((
            "2222222222222222222222222222222222222222".to_string(),
            ci(Some("success")),
        ));
        watch_end(&client, &watch).await;
    })
    .await;
    assert_eq!(repo.main_hold(&repository).await.expect("read"), None);
    assert_eq!(tasks.filed().len(), 1);
}

/// A watch with nothing held, whose base's checks never finish, ends once
/// the base's latest merge is hours old.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn an_unheld_watch_ends_hours_after_the_latest_merge() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    github.state().required = Some(RequiredChecks {
        checks: vec![RequiredCheck {
            name: "test".to_string(),
            app_id: None,
        }],
        complete: true,
    });
    github.state().base = Some((
        "2222222222222222222222222222222222222222".to_string(),
        ci(None),
    ));
    let tasks = FakeHoldTasks::default();
    let (watch, repository) = watching(&repo).await;
    repo.record_merge(
        watch.run_id,
        3,
        "https://github.com/macro/app/pull/3",
        chrono::Utc::now() - chrono::TimeDelta::hours(7),
    )
    .await
    .expect("merge");

    with_watch_worker(&repo, &github, &tasks, async |client, watches| {
        watches.watch(&watch).await.expect("start");
        watch_end(&client, &watch).await;
    })
    .await;
    assert_eq!(repo.main_hold(&repository).await.expect("read"), None);
}

/// A merge's signal that arrives while the watch's last look reads the base
/// makes it look again before it ends, so the merge is looked at.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_signal_during_the_last_look_makes_the_watch_look_again() {
    let repo = FakeRepo::default();
    let github = FakeGithub::default();
    github.state().required = Some(RequiredChecks {
        checks: vec![RequiredCheck {
            name: "test".to_string(),
            app_id: None,
        }],
        complete: true,
    });
    github.state().base = Some((
        "2222222222222222222222222222222222222222".to_string(),
        ci(Some("success")),
    ));
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    github.state().base_gate = Some(gate.clone());
    let tasks = FakeHoldTasks::default();
    let (watch, _) = watching(&repo).await;

    with_watch_worker(&repo, &github, &tasks, async |client, watches| {
        watches.watch(&watch).await.expect("start");
        // The first look reads the base, which would end the watch green.
        tokio::time::timeout(PATIENCE, github.base_read.notified())
            .await
            .expect("the watch looks");
        watches.watch(&watch).await.expect("signal");
        gate.add_permits(1);
        tokio::time::timeout(PATIENCE, github.base_read.notified())
            .await
            .expect("the watch looks again");
        gate.add_permits(1);
        watch_end(&client, &watch).await;
    })
    .await;
}
