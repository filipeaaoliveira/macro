//! An agent's questions to a Run's owner, on a running Temporal.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use factory_records::domain::models::{
    GateKind, NewRun, Outcome, Role, RunId, RunInsert, RunOutcome, StepKey,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{ReviewRoundOutcome, ReviewRoundRepository};
use factory_records::domain::standing_rules::RunRules;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use temporalio_client::errors::WorkflowGetResultError;
use temporalio_client::{
    Client, WorkflowCancelOptions, WorkflowGetResultOptions, WorkflowStartOptions,
};
use temporalio_sdk::Runtime;
use tokio::sync::Notify;
use uuid::Uuid;

use super::{
    AgentFailed, CodeAgent, PATIENCE, client, gates, julia, steps_of, walk_code_asking, watches,
    worker_on,
};
use crate::domain::fakes::{FakeGithub, FakeRepo, ci};
use crate::domain::flow::Flow;
use crate::domain::gate::AnswerGate;
use crate::domain::models::{AgentAnswer, AgentTask, AgentTurn, RunStart};
use crate::domain::ports::{AnswerDelivery, RunWorkflows, StepAgent};
use crate::domain::review_rounds::ReviewRoundsImpl;
use crate::domain::service::StepServiceImpl;
use crate::inbound::temporal::WorkerServices;
use crate::outbound::temporal_run_workflows::TemporalRunWorkflows;
use crate::temporal::FlowWorkflowDefinition;

/// An agent that asks the Run's owner a question for each of its first
/// `asks` tasks, and ends the next `done`. It keeps every task, and counts
/// the times it was stopped and let go.
#[derive(Clone)]
struct AskingAgent {
    asks: usize,
    tasks: Arc<Mutex<Vec<AgentTask>>>,
    stops: Arc<AtomicUsize>,
    releases: Arc<AtomicUsize>,
}

impl AskingAgent {
    fn asking(asks: usize) -> Self {
        Self {
            asks,
            tasks: Arc::default(),
            stops: Arc::default(),
            releases: Arc::default(),
        }
    }

    fn tasks(&self) -> Vec<AgentTask> {
        self.tasks.lock().expect("tasks").clone()
    }
}

impl StepAgent for AskingAgent {
    type Err = AgentFailed;

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, AgentFailed> {
        let asked = {
            let mut tasks = self.tasks.lock().expect("tasks");
            tasks.push(task);
            tasks.len()
        };
        if asked <= self.asks {
            return Ok(AgentAnswer {
                outcome: Outcome::question(),
                text: format!("Question {asked}: CSV or JSON?"),
            });
        }
        Ok(AgentAnswer {
            outcome: Outcome::done(),
            text: "The plan".to_string(),
        })
    }

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

/// The Run a test drives, and its records.
struct AskingRun {
    client: Client,
    run_id: RunId,
    repo: FakeRepo,
}

impl AskingRun {
    /// Wait for the Run's next Gate, and return its Step's place.
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

    /// Answer the question of the Step `seq` with `words`, as Julia, sent
    /// again while the workflow does not wait on the Gate yet.
    async fn answer(&self, seq: u16, words: &str) {
        let workflows = TemporalRunWorkflows::new(self.client.clone());
        let answer = AnswerGate {
            seq,
            answer: Outcome::answer(),
            note: Some(words.to_string()),
            user_id: julia(),
            approved_control_files: None,
            agent_session: None,
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
}

/// Walk a Run of one `plan` entry, whose agents may ask `questions`
/// questions a Step, with `agent`, running `drive` on the started
/// workflow; return how it ended and the Run's records.
async fn walk_plan<F>(
    agent: &AskingAgent,
    questions: u8,
    drive: impl FnOnce(AskingRun) -> F,
) -> (Result<RunOutcome, WorkflowGetResultError>, FakeRepo, RunId)
where
    F: Future<Output = ()>,
{
    let client = client().await;
    let repo = FakeRepo::default();
    let RunInsert::Inserted(run) = repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Export the report",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
        })
        .await
        .expect("insert")
    else {
        panic!("a run with no task is kept");
    };
    let mut flow = Flow::parse(
        json!({
            "entries": [{
                "id": "plan",
                "step": "plan",
                "role": "planner",
                "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
            }]
        }),
        |_| Some(crate::domain::flow::StepUse::Agent),
    )
    .expect("valid flow");
    flow.allow_questions(questions);
    let start = RunStart {
        run_id: run.id,
        flow,
    };
    let task_queue = format!("factory-test-{}", Uuid::now_v7());
    let runtime = Runtime::from_current_tokio(Default::default()).expect("runtime");
    let worker = worker_on(
        &task_queue,
        &runtime,
        client.clone(),
        WorkerServices {
            steps: StepServiceImpl::new(repo.clone(), steps_of(agent, &repo)),
            reviews: ReviewRoundsImpl::new(repo.clone(), FakeGithub::default(), repo.clone()),
            gates: gates(&repo),
            watches: watches(&repo),
        },
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
        drive(AskingRun {
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
    (ended, repo, run.id)
}

/// An agent's question ends its Step and opens a question Gate; the owner's
/// answer has the same Role go on in a Step of its own, at the next place
/// and the same visit, in the asking Step's session, with the answer. The
/// asking Step keeps its session for it: only the Step that went on lets
/// go of it.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn an_answered_question_has_the_same_role_go_on_with_the_answer() {
    let agent = AskingAgent::asking(1);

    let (ended, repo, run_id) = walk_plan(&agent, 2, |run| async move {
        let seq = run.next_gate().await;
        assert_eq!(seq, 1);
        run.answer(seq, "CSV, please").await;
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let steps = repo.list_steps(run_id).await.expect("steps");
    let ends: Vec<(u16, u16, &str)> = steps
        .iter()
        .map(|step| {
            let end = step.end.as_ref().expect("ended");
            (step.seq, step.visit, end.outcome.as_str())
        })
        .collect();
    assert_eq!(ends, [(1, 1, "question"), (2, 1, "done")]);
    let gate = steps[0].gate.as_ref().expect("the question's gate");
    assert_eq!(gate.kind, GateKind::Question);
    assert_eq!(gate.question, "Question 1: CSV or JSON?");
    assert_eq!(gate.answers, [Outcome::answer()]);
    let tasks = agent.tasks();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].resumes, None);
    assert_eq!(tasks[0].outcomes, [Outcome::done(), Outcome::question()]);
    assert_eq!(tasks[1].resumes, Some(1));
    assert!(tasks[1].prompt.contains("The owner's answer: CSV, please"));
    assert_eq!(agent.releases.load(Ordering::SeqCst), 1);
    assert_eq!(agent.stops.load(Ordering::SeqCst), 0);
}

/// A Step's work asks at most the Flow's questions: the next one goes to a
/// person as a stop, ending the Run `manual` with the question in the
/// Step's output, and the agents of the Steps holding its session are
/// stopped.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_question_past_the_step_s_questions_ends_the_run_for_a_person() {
    let agent = AskingAgent::asking(3);

    let (ended, repo, run_id) = walk_plan(&agent, 2, |run| async move {
        for answer in ["CSV", "Yes"] {
            let seq = run.next_gate().await;
            run.answer(seq, answer).await;
        }
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Manual)), "{ended:?}");
    let steps = repo.list_steps(run_id).await.expect("steps");
    assert_eq!(steps.len(), 3);
    assert!(steps.iter().all(|step| {
        step.end
            .as_ref()
            .is_some_and(|end| end.outcome == Outcome::question())
    }));
    assert!(steps[2].gate.is_none());
    assert_eq!(
        steps[2].end.as_ref().map(|end| end.output.as_str()),
        Some("Question 3: CSV or JSON?")
    );
    let tasks = agent.tasks();
    assert_eq!(
        tasks.iter().map(|task| task.resumes).collect::<Vec<_>>(),
        [None, Some(1), Some(2)]
    );
    // Every Step of the walk is held, the last one working in the first
    // one's session, so each is stopped.
    assert_eq!(agent.stops.load(Ordering::SeqCst), 3);
    assert_eq!(agent.releases.load(Ordering::SeqCst), 0);
}

/// A Run cancelled while its owner has not answered a question ends
/// cancelled, and the asking Step's agent, which kept its session, is
/// stopped.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_run_cancelled_on_a_question_stops_the_asking_step_s_agent() {
    let agent = AskingAgent::asking(1);

    let (ended, _repo, _run_id) = walk_plan(&agent, 2, |run| async move {
        run.next_gate().await;
        run.client
            .get_workflow_handle::<FlowWorkflowDefinition>(run.run_id.to_string())
            .cancel(WorkflowCancelOptions::builder().build())
            .await
            .expect("cancel");
    })
    .await;

    assert!(ended.is_err(), "{ended:?}");
    assert_eq!(agent.stops.load(Ordering::SeqCst), 1);
    assert_eq!(agent.releases.load(Ordering::SeqCst), 0);
}

/// A reviewer's question holds its round until the owner answers: the
/// reviewer goes on with the answer in a Step of its own, the others'
/// reviews are kept, and the round passes on all of them rather than
/// failing.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "Temporal doesn't exist in CI"]
async fn a_reviewer_s_question_holds_its_round_until_the_owner_answers() {
    let github = FakeGithub::default();
    github.state().checks = [ci(Some("success"))].into();
    let agent = CodeAgent::new(&github);
    *agent.reviews.lock().expect("reviews") = vec!["question", "pass", "pass", "pass"];
    let records = Arc::new(Mutex::new(None));

    let ended = walk_code_asking(&agent, &github, Some(2), |run| {
        let records = records.clone();
        async move {
            *records.lock().expect("records") = Some(run.repo.clone());
            run.answer_next_gate("approve").await;
            let asked = run.next_gate().await;
            run.answer_with(asked, "answer", Some("No trailing dash"))
                .await;
            run.answer_next_gate("approve").await;
        }
    })
    .await;

    assert!(matches!(ended, Ok(RunOutcome::Success)), "{ended:?}");
    let repo = records.lock().expect("records").clone().expect("records");
    let run_id = *repo.runs.lock().expect("runs").keys().next().expect("run");
    let steps = repo.list_steps(run_id).await.expect("steps");
    let reviewers: Vec<(u16, String)> = steps
        .iter()
        .filter(|step| step.entry.as_str() == "review")
        .map(|step| {
            let end = step.end.as_ref().expect("ended");
            (step.seq, end.outcome.to_string())
        })
        .collect();
    assert_eq!(reviewers.len(), 4);
    let asked: Vec<&(u16, String)> = reviewers
        .iter()
        .filter(|(_, outcome)| outcome == "question")
        .collect();
    assert_eq!(asked.len(), 1);
    assert_eq!(reviewers[3], (6, "pass".to_string()));
    let rounds = repo.review_rounds(run_id).await.expect("rounds");
    assert_eq!(rounds.len(), 1);
    assert_eq!(
        rounds[0].end.as_ref().map(|end| end.outcome),
        Some(ReviewRoundOutcome::Pass)
    );
    assert!(rounds[0].answered.contains(&6));
}
