//! The Flow workflow and its activities on a running Temporal, with the Run
//! records in memory and an agent the test drives.
//!
//! Run with: `TEMPORAL_ADDRESS=http://127.0.0.1:7233 cargo test -p factory_engine -- --ignored`
//! against the local stack's Temporal dev server (`just run_local`). Each
//! test runs its own worker on a task queue of its own, so the stack's
//! factory worker never takes its work.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_records::domain::models::{Outcome, Repository, RunId, RunInsert, RunOutcome, StepKey};
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
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::flow::Flow;
use crate::domain::gate::AnswerGate;
use crate::domain::models::{AgentAnswer, AgentTask, PullRequestState, RunStart};
use crate::domain::ports::{AnswerDelivery, Retryable, RunWorkflows, StepAgent};
use crate::domain::pull_request_steps::PullRequestSteps;
use crate::domain::service::StepServiceImpl;
use crate::outbound::temporal_run_workflows::TemporalRunWorkflows;
use crate::temporal::FlowWorkflowDefinition;

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

    async fn stop(
        &self,
        _key: StepKey,
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
) -> BuiltInSteps<A, FakeGithub, FakeRepo> {
    BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(FakeGithub::default(), repo.clone()),
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
        .insert_run(
            RunId::generate(),
            "plan",
            "Add a dark mode toggle",
            &julia,
            None,
            None,
        )
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
                "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
            }]
        }),
        |_| true,
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
        let worker = worker_on(&task_queue, &runtime, client.clone(), steps()).expect("worker");
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

    let last = worker_on(&task_queue, &runtime, client.clone(), steps()).expect("worker");
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
/// a review with the next of `reviews` (`pass` once they run out), and a CI
/// fix by pushing a commit whose checks pass. It says when it is asked for a
/// Step, and keeps the Steps it was asked for, by name.
#[derive(Clone)]
struct CodeAgent {
    github: FakeGithub,
    reviews: Arc<Mutex<Vec<&'static str>>>,
    steps: Arc<Mutex<Vec<String>>>,
    asked: Arc<Notify>,
}

/// The commit a CI fix pushes.
const FIXED_HEAD: &str = "2222222222222222222222222222222222222222";

impl CodeAgent {
    fn new(github: &FakeGithub) -> Self {
        Self {
            github: github.clone(),
            reviews: Arc::default(),
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
        let outcome = match step.as_str() {
            "review" => {
                let mut reviews = self.reviews.lock().expect("reviews");
                if reviews.is_empty() {
                    "pass"
                } else {
                    reviews.remove(0)
                }
            }
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

    async fn stop(
        &self,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), AgentFailed> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
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
    /// Gate is recorded just before the workflow waits on it, and an answer
    /// that lands in between is refused, as the API would refuse it: the
    /// answer is sent again, a few times at most.
    async fn answer(&self, seq: u16, answer: &str) {
        let workflows = TemporalRunWorkflows::new(self.client.clone());
        let answer = AnswerGate {
            seq,
            answer: Outcome::try_from(answer.to_string()).expect("valid"),
            note: None,
            user_id: julia(),
        };
        let mut delivery = AnswerDelivery::Refused("not sent".to_string());
        for _ in 0..50 {
            delivery = workflows
                .answer(self.run_id, &answer)
                .await
                .expect("answer");
            if delivery == AnswerDelivery::Taken {
                break;
            }
        }
        assert_eq!(delivery, AnswerDelivery::Taken);
    }

    /// Wait for the Run's next Gate, and give it `answer`.
    async fn answer_next_gate(&self, answer: &str) {
        let seq = self.next_gate().await;
        self.answer(seq, answer).await;
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
    let client = client().await;
    let repo = FakeRepo::default();
    let repository = Repository {
        url: "https://github.com/macro/app".to_string(),
        base_branch: "main".to_string(),
    };
    let RunInsert::Inserted(run) = repo
        .insert_run(
            RunId::generate(),
            "code",
            "Collapse repeated dashes in slugs",
            &julia(),
            None,
            Some(&repository),
        )
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let flow = BuiltInFlow::named("code")
        .expect("built in")
        .read(|_| true)
        .expect("valid flow");
    let start = RunStart {
        run_id: run.id,
        flow,
    };
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone()),
    );
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        StepServiceImpl::new(repo.clone(), steps),
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
    assert_eq!(agent.steps(), ["plan", "write", "review"]);
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
        ["plan", "write", "review", "ci_fix", "review"]
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
    assert_eq!(agent.steps(), ["plan", "write", "review", "fix", "review"]);
    let github = github.state();
    assert!(github.merges.is_empty());
    assert_eq!(github.opened, 1);
}

/// Reviews that keep asking for changes stop after three rounds: the pull
/// request is opened and checked, then left for a person, never merged.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_review_that_keeps_asking_for_changes_leaves_the_pull_request_for_a_person() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    *agent.reviews.lock().expect("reviews") = vec!["changes", "changes", "changes"];

    let ended = walk_code(&agent, &github, |run| async move {
        run.answer_next_gate("approve").await;
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Manual)), "{ended:?}");
    assert_eq!(
        agent.steps(),
        [
            "plan", "write", "review", "fix", "review", "fix", "review", "fix"
        ]
    );
    let github = github.state();
    assert_eq!(github.opened, 1);
    assert!(github.merges.is_empty());
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
