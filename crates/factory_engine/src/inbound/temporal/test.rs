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

use factory_records::domain::models::{Outcome, RunId, RunInsert, RunOutcome, StepKey};
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
use crate::domain::built_in_steps::BuiltInSteps;
use crate::domain::fakes::FakeRepo;
use crate::domain::flow::Flow;
use crate::domain::models::{AgentAnswer, AgentTask, RunStart};
use crate::domain::ports::{Retryable, StepAgent};
use crate::domain::service::StepServiceImpl;
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
    releases: Arc<AtomicUsize>,
}

impl Agent {
    fn doing(work: Work) -> Self {
        Self {
            work,
            asked: Arc::default(),
            tasks: Arc::default(),
            stops: Arc::default(),
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
        .insert_run(RunId::generate(), "Add a dark mode toggle", &julia, None)
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
    let client = client().await;
    let repo = FakeRepo::default();
    let start = plan_run(&repo).await;
    let run_id = start.run_id;
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        StepServiceImpl::new(repo.clone(), BuiltInSteps::new(agent.clone())),
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
/// stops the agent, once, and the Run and its Step end cancelled.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn cancelling_a_run_stops_its_working_step_s_agent_once() {
    let agent = Agent::doing(Work::Works);

    let (ended, repo, run_id) = walk(&agent, |client, run_id| {
        let agent = agent.clone();
        async move {
            asked(&agent).await;
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
    let run = repo.get_run(run_id).await.expect("read").expect("kept");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Cancelled));
    let steps = repo.list_steps(run_id).await.expect("read");
    assert_eq!(
        steps[0].end.as_ref().map(|end| end.outcome.as_str()),
        Some("cancelled")
    );
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
    let steps = || StepServiceImpl::new(repo.clone(), BuiltInSteps::new(agent.clone()));
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
