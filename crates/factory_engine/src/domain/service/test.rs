use factory_records::domain::standing_rules::RunRules;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use factory_records::domain::models::{
    AgentName, EntryId, HandBack, HandedBack, MAX_QUESTION_BYTES, NewRun, Outcome, Role, RunId,
    RunInsert, RunOutcome, StepKey, StepSession,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{DEFAULT_QUESTIONS, FactoryServiceImpl, StepServiceImpl};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::flow::{StepName, StepUse};
use crate::domain::gate::{AnswerGate, ReplyGate};
use crate::domain::models::{
    Answerer, FactoryError, RunFinish, RunStart, SessionSteer, StepEnding, StepError, StepInput,
    StepReport, StepRequest, StepStart,
};
use crate::domain::ports::{
    AgentFailure, AnswerDelivery, FactoryService, Retryable, RunWorkflows, RunWorkflowsError,
    StepExecutor, StepService,
};

mod automations;
mod board;
mod control_files;
mod definition;
mod evaluation;
mod gate_owner;
mod orchestrator;
mod standing_rules;
mod step_terminal;
mod task_thread;

/// Keeps what it was sent. Refuses every answer when `refuse` is set; a
/// start fails when `fail` is set, and leaves its workflow behind when
/// `started_anyway` is set too.
#[derive(Clone, Default)]
struct FakeWorkflows {
    started: Arc<Mutex<Vec<RunStart>>>,
    answers: Arc<Mutex<Vec<AnswerGate>>>,
    replies: Arc<Mutex<Vec<ReplyGate>>>,
    cancelled: Arc<Mutex<Vec<RunId>>>,
    fail: bool,
    started_anyway: bool,
    refuse: bool,
    /// The workflow completed, though the Run's end is not recorded yet.
    completed: bool,
    /// The workflow never started.
    unstarted: bool,
    /// What the next cancels and existence checks answer, ahead of the
    /// flags above: a workflow that starts or completes between two calls.
    cancel_answers: Arc<Mutex<VecDeque<bool>>>,
    exists_answers: Arc<Mutex<VecDeque<bool>>>,
}

impl RunWorkflows for FakeWorkflows {
    async fn start(&self, start: &RunStart) -> Result<(), RunWorkflowsError> {
        if self.fail {
            return Err(RunWorkflowsError::Other(rootcause::report!("refused")));
        }
        self.started.lock().expect("started").push(start.clone());
        Ok(())
    }

    async fn exists(&self, _run_id: RunId) -> Result<bool, RunWorkflowsError> {
        let scripted = self.exists_answers.lock().expect("exists").pop_front();
        Ok(scripted.unwrap_or(self.started_anyway || self.completed))
    }

    async fn answer(
        &self,
        _run_id: RunId,
        answer: &AnswerGate,
    ) -> Result<AnswerDelivery, RunWorkflowsError> {
        if self.refuse {
            return Ok(AnswerDelivery::Refused("not waiting".to_string()));
        }
        self.answers.lock().expect("answers").push(answer.clone());
        Ok(AnswerDelivery::Taken)
    }

    async fn reply(
        &self,
        _run_id: RunId,
        reply: &ReplyGate,
    ) -> Result<AnswerDelivery, RunWorkflowsError> {
        if self.refuse {
            return Ok(AnswerDelivery::Refused("not waiting".to_string()));
        }
        self.replies.lock().expect("replies").push(reply.clone());
        Ok(AnswerDelivery::Taken)
    }

    async fn cancel(&self, run_id: RunId) -> Result<bool, RunWorkflowsError> {
        self.cancelled.lock().expect("cancelled").push(run_id);
        let scripted = self.cancel_answers.lock().expect("cancels").pop_front();
        Ok(scripted.unwrap_or(!self.completed && !self.unstarted))
    }
}

/// Serves `reply`, which ends `done` with a fixed output, or fails with
/// `failure` when it is set, and keeps the places of the earlier Steps each
/// run was given, and the owner's notes it was given.
#[derive(Clone)]
struct FakeExecutor {
    output: String,
    failure: Option<WorkFailure>,
    given: Arc<Mutex<Vec<Vec<u16>>>>,
    noted: Arc<Mutex<Vec<Vec<String>>>>,
    /// Whether it serves every Step, not only `reply`.
    serves_all: bool,
    /// What the Step Terminal asked of the sessions Steps work in, in order.
    steered: Arc<Mutex<Vec<(StepKey, Uuid, SessionSteer)>>>,
    /// Whether the sessions refuse it.
    steer_refused: bool,
}

impl Default for FakeExecutor {
    fn default() -> Self {
        Self {
            output: "Replied".to_string(),
            failure: None,
            given: Arc::default(),
            noted: Arc::default(),
            serves_all: false,
            steered: Arc::default(),
            steer_refused: false,
        }
    }
}

/// A Step's work that failed, for good or not.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("the work failed")]
struct WorkFailure {
    retryable: bool,
    out_of_quota: bool,
    overdue: bool,
}

impl Retryable for WorkFailure {
    fn retryable(&self) -> bool {
        self.retryable
    }
}

impl AgentFailure for WorkFailure {
    fn out_of_quota(&self) -> bool {
        self.out_of_quota
    }

    fn overdue(&self) -> bool {
        self.overdue
    }
}

impl StepExecutor for FakeExecutor {
    type Err = WorkFailure;

    fn serves(&self, step: &StepName) -> Option<StepUse> {
        (self.serves_all || step.as_str() == "reply").then_some(StepUse::Work)
    }

    async fn run(&self, _step: &StepName, input: StepInput<'_>) -> Result<StepReport, WorkFailure> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let earlier = input.earlier.iter().map(|step| step.seq).collect();
        self.given.lock().expect("given").push(earlier);
        let notes = input
            .owner_notes
            .iter()
            .map(|note| note.text.clone())
            .collect();
        self.noted.lock().expect("noted").push(notes);
        Ok(StepReport {
            outcome: Outcome::done(),
            output: self.output.clone(),
        })
    }

    async fn stop(
        &self,
        _step: &StepName,
        _role: Option<&Role>,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), WorkFailure> {
        Ok(())
    }

    async fn release(
        &self,
        _step: &StepName,
        _role: Option<&Role>,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), WorkFailure> {
        Ok(())
    }

    async fn steer(
        &self,
        _role: &Role,
        key: StepKey,
        _owner: &MacroUserIdStr<'static>,
        session: &StepSession,
        steer: SessionSteer,
    ) -> Result<(), WorkFailure> {
        if self.steer_refused {
            return Err(WorkFailure {
                retryable: false,
                out_of_quota: false,
                overdue: false,
            });
        }
        self.steered
            .lock()
            .expect("steered")
            .push((key, session.session_id, steer));
        Ok(())
    }
}

fn julia() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid")
}

fn teo() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|teo@example.com".to_string()).expect("valid")
}

fn nobody() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from("macro|nobody@example.com".to_string()).expect("valid")
}

/// Keep for the Run `run_id` a Flow whose `reply` entry has a notify-only
/// Gate, as a Run started on such a Flow keeps it.
fn keep_notify_only_reply(repo: &FakeRepo, run_id: RunId) {
    let mut flow = crate::domain::built_in_flows::BuiltInFlow::named("code")
        .expect("built in")
        .read(|_| Some(StepUse::Work))
        .expect("valid flow");
    let entry = &mut flow.entries[0];
    entry.id = EntryId::try_from("reply".to_string()).expect("valid");
    entry.gate = Some(crate::domain::flow::GateDefinition {
        question: "The plan is ready.".to_string(),
        answers: Vec::new(),
        after: None,
        kind: None,
        person_only: false,
        notify_only: true,
    });
    let kept = serde_json::to_value(&flow).expect("flows serialize");
    repo.kept_flows
        .lock()
        .expect("kept flows")
        .insert(run_id, (kept, false));
}

fn reply_step(run_id: RunId, seq: u16) -> StepRequest {
    StepRequest {
        key: StepKey { run_id, seq },
        entry: EntryId::try_from("reply".to_string()).expect("valid"),
        step: StepName::try_from("reply".to_string()).expect("valid"),
        role: None,
        visit: 1,
        round: None,
        may_ask: false,
        questions_left: 0,
        resumes: None,
    }
}

/// The Run API over `repo` and `workflows`, serving `reply` only.
fn service(
    repo: &FakeRepo,
    workflows: &FakeWorkflows,
) -> FactoryServiceImpl<FakeRepo, FakeWorkflows, FakeExecutor, FakeRepo, FakeRepo, FakeGithub> {
    FactoryServiceImpl::new(
        repo.clone(),
        workflows.clone(),
        FakeExecutor::default(),
        repo.clone(),
        repo.clone(),
        FakeGithub::default(),
    )
}

/// The Run API over `repo`, `workflows` and `github`, serving every Step.
fn code_service(
    repo: &FakeRepo,
    workflows: &FakeWorkflows,
    github: &FakeGithub,
) -> FactoryServiceImpl<FakeRepo, FakeWorkflows, FakeExecutor, FakeRepo, FakeRepo, FakeGithub> {
    FactoryServiceImpl::new(
        repo.clone(),
        workflows.clone(),
        FakeExecutor {
            serves_all: true,
            ..FakeExecutor::default()
        },
        repo.clone(),
        repo.clone(),
        github.clone(),
    )
}

/// A Run of Julia's, kept with no task.
async fn julia_s_run(repo: &FakeRepo) -> RunId {
    match repo
        .insert_run(NewRun {
            id: RunId::generate(),
            flow: "plan",
            request: "Plan",
            user_id: &julia(),
            task_id: None,
            project_id: None,
            repository: None,
            created_in_person: false,
            control_file_patterns: &[],
            control_files_team_id: None,
            standing_rules: RunRules::DEFAULT,
            automation: None,
            definition: None,
            kept_flow: None,
        })
        .await
        .expect("insert")
    {
        RunInsert::Inserted(run) => run.id,
        RunInsert::NoSuchTask
        | RunInsert::TaskBusy
        | RunInsert::FiringStarted
        | RunInsert::QueueFull(_) => {
            panic!("a run with no task is kept")
        }
    }
}

#[tokio::test]
async fn creating_a_run_records_it_for_its_owner_and_starts_its_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = service(&repo, &workflows);

    let created = service
        .create_run(
            julia(),
            "reply".to_string(),
            "Summarise the release notes".to_string(),
            None,
            None,
            true,
        )
        .await
        .expect("create");

    let started = workflows.started.lock().expect("started").clone();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].run_id, created.run.id);
    assert_eq!(created.run.user_id, julia());
    let read = service
        .get_run(&julia(), created.run.id)
        .await
        .expect("get");
    assert_eq!(read.run, created.run);
}

/// A `code` Run works on the repository it names, from the branch that
/// repository's GitHub says is its default.
#[tokio::test]
async fn a_code_run_keeps_its_repository_and_its_base_branch() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();

    let created = code_service(&repo, &workflows, &FakeGithub::default())
        .create_run(
            julia(),
            "code".to_string(),
            "Collapse repeated dashes".to_string(),
            None,
            Some("https://github.com/macro/app".to_string()),
            true,
        )
        .await
        .expect("create");

    let repository = created.run.repository.expect("kept");
    assert_eq!(repository.url, "https://github.com/macro/app");
    assert_eq!(repository.base_branch, "main");
    assert_eq!(workflows.started.lock().expect("started").len(), 1);
}

/// A Flow that works on a repository needs one its owner reaches and may
/// push to with their GitHub account, and the others take none; nothing is
/// kept otherwise.
#[tokio::test]
async fn a_run_s_repository_must_fit_its_flow_and_be_reachable() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let unreachable = FakeGithub::default();
    unreachable.state().unreachable = true;
    let read_only = FakeGithub::default();
    read_only.state().read_only = true;
    let create = |flow: &str, repo_url: Option<&str>, github: &FakeGithub| {
        let service = code_service(&repo, &workflows, github);
        let flow = flow.to_string();
        let repo_url = repo_url.map(str::to_string);
        async move {
            service
                .create_run(
                    julia(),
                    flow,
                    "Collapse repeated dashes".to_string(),
                    None,
                    repo_url,
                    true,
                )
                .await
        }
    };

    let none = create("code", None, &FakeGithub::default()).await;
    let unused = create(
        "reply",
        Some("https://github.com/macro/app"),
        &FakeGithub::default(),
    )
    .await;
    let invalid = create(
        "code",
        Some("https://github.com/macro/app/tree/main"),
        &FakeGithub::default(),
    )
    .await;
    let out_of_reach = create("code", Some("https://github.com/macro/app"), &unreachable).await;
    let not_pushable = create("code", Some("https://github.com/macro/app"), &read_only).await;

    assert!(matches!(none, Err(FactoryError::RepositoryRequired)));
    assert!(matches!(unused, Err(FactoryError::RepositoryNotUsed)));
    assert!(matches!(invalid, Err(FactoryError::InvalidRepository)));
    assert!(matches!(
        out_of_reach,
        Err(FactoryError::UnreachableRepository)
    ));
    assert!(matches!(not_pushable, Err(FactoryError::NoPushAccess)));
    assert!(repo.runs.lock().expect("runs").is_empty());
    assert!(workflows.started.lock().expect("started").is_empty());
}

#[tokio::test]
async fn a_run_whose_workflow_does_not_start_is_not_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows {
        fail: true,
        ..FakeWorkflows::default()
    };

    let refused = service(&repo, &workflows)
        .create_run(
            julia(),
            "reply".to_string(),
            "Summarise".to_string(),
            None,
            None,
            true,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::Internal(_))));
    assert!(repo.runs.lock().expect("runs").is_empty());
}

/// A start that fails after the server took it leaves a workflow behind,
/// which records its Steps against the Run: the Run must stay.
#[tokio::test]
async fn a_run_whose_workflow_started_despite_a_failed_start_is_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows {
        fail: true,
        started_anyway: true,
        ..FakeWorkflows::default()
    };

    let refused = service(&repo, &workflows)
        .create_run(
            julia(),
            "reply".to_string(),
            "Summarise".to_string(),
            None,
            None,
            true,
        )
        .await;

    assert!(refused.is_err());
    assert_eq!(repo.runs.lock().expect("runs").len(), 1);
}

/// The built-in `plan` Flow names Steps this worker does not serve.
#[tokio::test]
async fn a_flow_with_a_step_no_worker_serves_is_refused_before_anything_is_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();

    let refused = service(&repo, &workflows)
        .create_run(
            julia(),
            "plan".to_string(),
            "Plan the migration".to_string(),
            None,
            None,
            true,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::InvalidFlow(_))));
    assert!(repo.runs.lock().expect("runs").is_empty());
    assert!(workflows.started.lock().expect("started").is_empty());
}

#[tokio::test]
async fn a_flow_no_built_in_flow_is_named_for_is_refused_before_anything_is_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();

    let refused = service(&repo, &workflows)
        .create_run(
            julia(),
            "deploy".to_string(),
            "Deploy the release".to_string(),
            None,
            None,
            true,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::UnknownFlow)));
    assert!(repo.runs.lock().expect("runs").is_empty());
    assert!(workflows.started.lock().expect("started").is_empty());
}

#[tokio::test]
async fn an_empty_request_is_refused() {
    let refused = service(&FakeRepo::default(), &FakeWorkflows::default())
        .create_run(
            julia(),
            "reply".to_string(),
            "  ".to_string(),
            None,
            None,
            true,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::EmptyRequest)));
}

#[tokio::test]
async fn a_run_for_a_user_with_no_account_is_refused() {
    let repo = FakeRepo::default();

    let refused = service(&repo, &FakeWorkflows::default())
        .create_run(
            nobody(),
            "reply".to_string(),
            "Fix the login bug".to_string(),
            None,
            None,
            true,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::UnknownUser)));
    assert!(repo.runs.lock().expect("runs").is_empty());
}

#[tokio::test]
async fn someone_else_s_run_reads_answers_and_cancels_as_missing() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = julia_s_run(&repo).await;
    let service = service(&repo, &workflows);

    let read = service.get_run(&teo(), run_id).await;
    let answered = service
        .answer_gate(
            teo(),
            run_id,
            1,
            Outcome::done(),
            None,
            None,
            Answerer::InPerson,
        )
        .await;
    let cancelled = service.cancel_run(&teo(), run_id).await;
    let linked = service.link_task(&teo(), run_id, Uuid::now_v7()).await;

    assert!(matches!(read, Err(FactoryError::NotFound)));
    assert!(matches!(answered, Err(FactoryError::NotFound)));
    assert!(matches!(cancelled, Err(FactoryError::NotFound)));
    assert!(matches!(linked, Err(FactoryError::NotFound)));
    assert!(workflows.answers.lock().expect("answers").is_empty());
    assert!(workflows.cancelled.lock().expect("cancelled").is_empty());
}

#[tokio::test]
async fn an_answer_is_sent_to_the_run_s_workflow_as_its_owner() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = julia_s_run(&repo).await;

    service(&repo, &workflows)
        .answer_gate(
            julia(),
            run_id,
            1,
            Outcome::done(),
            Some("Go ahead".to_string()),
            None,
            Answerer::InPerson,
        )
        .await
        .expect("answer");

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(
        answers,
        [AnswerGate {
            seq: 1,
            answer: Outcome::done(),
            note: Some("Go ahead".to_string()),
            user_id: julia(),
            approved_control_files: None,
            agent_session: None,
        }]
    );
}

#[tokio::test]
async fn an_answer_the_workflow_refuses_is_a_conflict() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows {
        refuse: true,
        ..FakeWorkflows::default()
    };
    let run_id = julia_s_run(&repo).await;

    let refused = service(&repo, &workflows)
        .answer_gate(
            julia(),
            run_id,
            1,
            Outcome::done(),
            None,
            None,
            Answerer::InPerson,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::AnswerRefused(_))));
}

#[tokio::test]
async fn an_answer_from_a_user_with_no_account_never_reaches_the_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = julia_s_run(&repo).await;

    let refused = service(&repo, &workflows)
        .answer_gate(
            nobody(),
            run_id,
            1,
            Outcome::done(),
            None,
            None,
            Answerer::InPerson,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::UnknownUser)));
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

#[tokio::test]
async fn a_note_larger_than_a_gate_keeps_is_refused() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = julia_s_run(&repo).await;

    let refused = service(&repo, &workflows)
        .answer_gate(
            julia(),
            run_id,
            1,
            Outcome::done(),
            Some("x".repeat(4_097)),
            None,
            Answerer::InPerson,
        )
        .await;

    assert!(matches!(refused, Err(FactoryError::NoteTooLarge { .. })));
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

#[tokio::test]
async fn cancelling_a_run_twice_is_one_cancel_and_an_ended_run_is_not_cancelled() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let cancelled_run = julia_s_run(&repo).await;
    let finished_run = julia_s_run(&repo).await;
    let service = service(&repo, &workflows);

    service
        .cancel_run(&julia(), cancelled_run)
        .await
        .expect("cancel");
    repo.end_run(cancelled_run, RunOutcome::Cancelled)
        .await
        .expect("the workflow ends it");
    let again = service.cancel_run(&julia(), cancelled_run).await;
    repo.end_run(finished_run, RunOutcome::Success)
        .await
        .expect("end");
    let finished = service.cancel_run(&julia(), finished_run).await;

    assert!(again.is_ok());
    assert!(matches!(finished, Err(FactoryError::RunEnded)));
    assert_eq!(
        *workflows.cancelled.lock().expect("cancelled"),
        [cancelled_run]
    );
}

#[tokio::test]
async fn a_run_created_for_a_task_keeps_it() {
    let repo = FakeRepo::default();
    let task_id = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task_id);
    let service = service(&repo, &FakeWorkflows::default());

    let created = service
        .create_run(
            julia(),
            "reply".to_string(),
            "Fix the login bug".to_string(),
            Some(task_id),
            None,
            true,
        )
        .await
        .expect("create");

    assert_eq!(created.run.task_id, Some(task_id));
}

#[tokio::test]
async fn a_run_for_a_document_that_is_not_a_task_or_is_missing_is_refused() {
    let repo = FakeRepo::default();
    let note = Uuid::now_v7();
    repo.documents.lock().expect("documents").insert(note);
    let workflows = FakeWorkflows::default();
    let service = service(&repo, &workflows);

    let not_a_task = service
        .create_run(
            julia(),
            "reply".to_string(),
            "Fix".to_string(),
            Some(note),
            None,
            true,
        )
        .await;
    let missing = service
        .create_run(
            julia(),
            "reply".to_string(),
            "Fix".to_string(),
            Some(Uuid::now_v7()),
            None,
            true,
        )
        .await;

    assert!(matches!(not_a_task, Err(FactoryError::NotATask)));
    assert!(matches!(missing, Err(FactoryError::UnknownTask)));
    assert!(repo.runs.lock().expect("runs").is_empty());
    assert!(workflows.started.lock().expect("started").is_empty());
}

#[tokio::test]
async fn linking_a_run_replaces_its_task() {
    let repo = FakeRepo::default();
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").extend([first, second]);
    let run_id = julia_s_run(&repo).await;
    let service = service(&repo, &FakeWorkflows::default());

    service
        .link_task(&julia(), run_id, first)
        .await
        .expect("link");
    let relinked = service
        .link_task(&julia(), run_id, second)
        .await
        .expect("relink");

    assert_eq!(relinked.run.task_id, Some(second));
}

#[tokio::test]
async fn running_a_step_reads_the_run_and_records_nothing() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let executor = FakeExecutor::default();
    let steps = StepServiceImpl::new(repo.clone(), executor.clone());

    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    let report = steps.run_step(reply_step(run_id, 1)).await.expect("run");

    assert_eq!(report.output, "Replied");
    let recorded = repo.list_steps(run_id).await.expect("steps");
    assert_eq!(recorded[0].end, None);
}

#[tokio::test]
async fn a_step_is_given_the_steps_before_it_that_ended() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let executor = FakeExecutor::default();
    let steps = StepServiceImpl::new(repo.clone(), executor.clone());

    for seq in [1, 2] {
        steps
            .start_step(reply_step(run_id, seq))
            .await
            .expect("start");
        let report = steps.run_step(reply_step(run_id, seq)).await.expect("run");
        steps
            .end_step(StepEnding {
                key: StepKey { run_id, seq },
                report,
            })
            .await
            .expect("end");
    }

    let given = executor.given.lock().expect("given").clone();
    assert_eq!(given, [Vec::<u16>::new(), vec![1]]);
}

#[tokio::test]
async fn a_step_whose_output_is_too_large_fails_without_recording_its_output() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let executor = FakeExecutor {
        output: "x".repeat(65_537),
        ..FakeExecutor::default()
    };
    let steps = StepServiceImpl::new(repo, executor);

    let refused = steps.run_step(reply_step(run_id, 1)).await;

    assert!(matches!(
        refused,
        Err(crate::domain::models::StepError::OutputTooLarge { .. })
    ));
}

#[tokio::test]
async fn ending_a_run_records_its_outcome() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());

    steps
        .end_run(RunFinish {
            run_id,
            outcome: RunOutcome::Success,
        })
        .await
        .expect("end");

    let ended = repo.get_run(run_id).await.expect("get").expect("exists");
    assert_eq!(ended.end.map(|end| end.outcome), Some(RunOutcome::Success));
}

/// A Run of Julia's whose first Step works in a claimed session offering
/// `done`.
async fn step_in_a_session(repo: &FakeRepo) -> (StepKey, Uuid) {
    let run_id = julia_s_run(repo).await;
    let key = StepKey { run_id, seq: 1 };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    let session = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: Some(AgentName::try_from("primary".to_string()).expect("valid")),
        bot: None,
    };
    let session_id = session.session_id;
    let planner = Role::try_from("planner".to_string()).expect("valid");
    repo.claim_session(key, session, &planner, &[Outcome::done()])
        .await
        .expect("claim");
    (key, session_id)
}

fn handing_back(outcome: Outcome) -> HandBack {
    HandBack {
        outcome,
        output: "The plan".to_string(),
    }
}

#[tokio::test]
async fn the_owner_s_session_hands_back_an_offered_outcome() {
    let repo = FakeRepo::default();
    let (key, session_id) = step_in_a_session(&repo).await;

    service(&repo, &FakeWorkflows::default())
        .hand_back(session_id, julia(), handing_back(Outcome::done()))
        .await
        .expect("hand back");

    let first_prompt = repo
        .claimed_session(key)
        .await
        .expect("read")
        .expect("claimed")
        .action_id;
    let handed = repo.handed_back(key, first_prompt).await.expect("read");
    assert_eq!(handed, HandedBack::Handed(handing_back(Outcome::done())));
}

#[tokio::test]
async fn a_hand_back_is_refused_from_another_user_another_session_or_with_another_outcome() {
    let repo = FakeRepo::default();
    let (_, session_id) = step_in_a_session(&repo).await;
    let service = service(&repo, &FakeWorkflows::default());

    let other_user = service
        .hand_back(session_id, teo(), handing_back(Outcome::done()))
        .await;
    let other_session = service
        .hand_back(Uuid::now_v7(), julia(), handing_back(Outcome::done()))
        .await;
    let other_outcome = service
        .hand_back(session_id, julia(), handing_back(Outcome::unchanged()))
        .await;

    assert!(matches!(other_user, Err(FactoryError::NotTheOwner)));
    assert!(matches!(other_session, Err(FactoryError::NotAStepSession)));
    assert!(matches!(
        other_outcome,
        Err(FactoryError::OutcomeNotOffered { .. })
    ));
}

#[tokio::test]
async fn an_answer_the_gate_does_not_offer_never_reaches_the_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = julia_s_run(&repo).await;
    let key = StepKey { run_id, seq: 1 };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.open_gate(key, "Approve?", &[Outcome::done()])
        .await
        .expect("open");

    let refused = service(&repo, &workflows)
        .answer_gate(
            julia(),
            run_id,
            1,
            Outcome::unchanged(),
            None,
            None,
            Answerer::InPerson,
        )
        .await;

    assert!(matches!(
        refused,
        Err(FactoryError::AnswerNotOffered { .. })
    ));
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

#[tokio::test]
async fn work_that_fails_for_good_is_not_retried_and_other_failures_are() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let for_good = StepServiceImpl::new(
        repo.clone(),
        FakeExecutor {
            failure: Some(WorkFailure {
                retryable: false,
                out_of_quota: false,
                overdue: false,
            }),
            ..FakeExecutor::default()
        },
    );
    let for_now = StepServiceImpl::new(
        repo.clone(),
        FakeExecutor {
            failure: Some(WorkFailure {
                retryable: true,
                out_of_quota: false,
                overdue: false,
            }),
            ..FakeExecutor::default()
        },
    );
    let overdue = StepServiceImpl::new(
        repo.clone(),
        FakeExecutor {
            failure: Some(WorkFailure {
                retryable: true,
                out_of_quota: false,
                overdue: true,
            }),
            ..FakeExecutor::default()
        },
    );
    let out_of_quota = StepServiceImpl::new(
        repo.clone(),
        FakeExecutor {
            failure: Some(WorkFailure {
                retryable: false,
                out_of_quota: true,
                overdue: false,
            }),
            ..FakeExecutor::default()
        },
    );

    let final_failure = for_good.run_step(reply_step(run_id, 1)).await;
    let passing_failure = for_now.run_step(reply_step(run_id, 1)).await;
    let quota_failure = out_of_quota.run_step(reply_step(run_id, 1)).await;
    let overdue_failure = overdue.run_step(reply_step(run_id, 1)).await;

    assert!(matches!(
        final_failure,
        Err(crate::domain::models::StepError::Final(_))
    ));
    assert!(matches!(
        passing_failure,
        Err(crate::domain::models::StepError::Internal(_))
    ));
    // A quota that ran out is infrastructure's, which the workflow reads.
    assert!(matches!(
        quota_failure,
        Err(crate::domain::models::StepError::OutOfQuota(_))
    ));
    // An agent still at work when the wait ended is the work's, retried.
    assert!(matches!(
        overdue_failure,
        Err(crate::domain::models::StepError::Overdue(_))
    ));
}

#[tokio::test]
async fn cancelling_a_run_whose_workflow_completed_is_a_conflict() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows {
        completed: true,
        ..FakeWorkflows::default()
    };
    let run_id = julia_s_run(&repo).await;

    let refused = service(&repo, &workflows)
        .cancel_run(&julia(), run_id)
        .await;

    assert!(matches!(refused, Err(FactoryError::RunEnded)));
}

/// A Run whose workflow never started has nothing to stop: cancelling it
/// ends it, and a workflow that starts late takes no Step of it.
#[tokio::test]
async fn cancelling_a_run_whose_workflow_never_started_ends_it_cancelled() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows {
        unstarted: true,
        ..FakeWorkflows::default()
    };
    let run_id = julia_s_run(&repo).await;

    service(&repo, &workflows)
        .cancel_run(&julia(), run_id)
        .await
        .expect("cancel");
    let started = StepServiceImpl::new(repo.clone(), FakeExecutor::default())
        .start_step(reply_step(run_id, 1))
        .await;

    let run = repo.get_run(run_id).await.expect("read").expect("kept");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Cancelled));
    assert_eq!(
        started.ok(),
        Some(StepStart::RunEnded(RunOutcome::Cancelled))
    );
    assert!(repo.list_steps(run_id).await.expect("read").is_empty());
}

/// Workflows that answer cancels with `cancels` and existence checks with
/// `exists`, in turn.
fn scripted(cancels: &[bool], exists: &[bool]) -> FakeWorkflows {
    FakeWorkflows {
        cancel_answers: Arc::new(Mutex::new(cancels.iter().copied().collect())),
        exists_answers: Arc::new(Mutex::new(exists.iter().copied().collect())),
        ..FakeWorkflows::default()
    }
}

/// A workflow that starts just as its Run is cancelled, found there once
/// the Run's end is recorded, is cancelled too.
#[tokio::test]
async fn a_workflow_that_starts_as_its_run_is_cancelled_is_cancelled_too() {
    let repo = FakeRepo::default();
    let workflows = scripted(&[false, true], &[false, true]);
    let run_id = julia_s_run(&repo).await;

    service(&repo, &workflows)
        .cancel_run(&julia(), run_id)
        .await
        .expect("cancel");

    let run = repo.get_run(run_id).await.expect("read").expect("kept");
    assert_eq!(run.end.map(|end| end.outcome), Some(RunOutcome::Cancelled));
    assert_eq!(workflows.cancelled.lock().expect("cancelled").len(), 2);
}

/// A workflow that started between the cancel and the check is running,
/// not ended: it is cancelled, never answered with a conflict.
#[tokio::test]
async fn a_workflow_that_started_after_the_cancel_was_sent_is_cancelled() {
    let repo = FakeRepo::default();
    let workflows = scripted(&[false, true], &[true]);
    let run_id = julia_s_run(&repo).await;

    service(&repo, &workflows)
        .cancel_run(&julia(), run_id)
        .await
        .expect("cancel");

    let run = repo.get_run(run_id).await.expect("read").expect("kept");
    assert_eq!(run.end, None, "its workflow records the end");
    assert_eq!(workflows.cancelled.lock().expect("cancelled").len(), 2);
}

/// A Run that ended does no more work, whichever attempt of a Step asks.
#[tokio::test]
async fn a_step_of_a_run_that_ended_does_no_work() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let executor = FakeExecutor::default();
    let steps = StepServiceImpl::new(repo.clone(), executor.clone());
    steps
        .start_step(reply_step(run_id, 1))
        .await
        .expect("start");
    repo.end_run(run_id, RunOutcome::Cancelled)
        .await
        .expect("end");

    let worked = steps.run_step(reply_step(run_id, 1)).await;

    assert!(matches!(worked, Err(StepError::RunEnded)), "{worked:?}");
    assert!(executor.given.lock().expect("given").is_empty());
}

/// A Run kept when Temporal could not say whether its workflow started,
/// and whose workflow never came, ends `infrastructure` once it is old
/// enough, as none of its work ran; a new Run waiting for its first Step
/// is left alone.
#[tokio::test]
async fn reading_a_run_whose_workflow_never_started_ends_it() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let stuck = julia_s_run(&repo).await;
    let new = julia_s_run(&repo).await;
    if let Some(run) = repo.runs.lock().expect("runs").get_mut(&stuck) {
        run.created_at -= chrono::TimeDelta::minutes(10);
    }
    let service = service(&repo, &workflows);

    let stuck = service.get_run(&julia(), stuck).await.expect("read");
    let new = service.get_run(&julia(), new).await.expect("read");

    assert_eq!(
        stuck.run.end.map(|end| end.outcome),
        Some(RunOutcome::Infrastructure)
    );
    assert_eq!(new.run.end, None);
}

/// Only the Flows whose Steps the worker serves are offered: this one
/// serves `reply` alone.
#[tokio::test]
async fn only_the_flows_the_worker_runs_are_offered() {
    let flows = service(&FakeRepo::default(), &FakeWorkflows::default())
        .list_flows(&julia())
        .await
        .expect("flows");

    let names: Vec<&str> = flows.iter().map(|flow| flow.name.as_str()).collect();
    assert_eq!(names, ["reply"]);
    assert_eq!(
        flows[0].description,
        "Replies to the Run's request in one Step."
    );
}

/// A task's list holds its owner's 50 newest Runs, however many they
/// started on it.
#[tokio::test]
async fn a_task_s_list_holds_its_owner_s_fifty_newest_runs() {
    let repo = FakeRepo::default();
    let task_id = Uuid::now_v7();
    repo.tasks.lock().expect("tasks").insert(task_id);
    let mut started = Vec::new();
    for n in 0..51 {
        match repo
            .insert_run(NewRun {
                id: RunId::generate(),
                flow: "plan",
                request: &format!("Run {n}"),
                user_id: &julia(),
                task_id: Some(task_id),
                project_id: None,
                repository: None,
                created_in_person: false,
                control_file_patterns: &[],
                control_files_team_id: None,
                standing_rules: RunRules::DEFAULT,
                automation: None,
                definition: None,
                kept_flow: None,
            })
            .await
            .expect("insert")
        {
            RunInsert::Inserted(run) => started.push(run.id),
            RunInsert::NoSuchTask
            | RunInsert::TaskBusy
            | RunInsert::FiringStarted
            | RunInsert::QueueFull(_) => {
                panic!("the task exists")
            }
        }
    }

    let listed = service(&repo, &FakeWorkflows::default())
        .list_task_runs(&julia(), task_id)
        .await
        .expect("list");

    let listed: Vec<RunId> = listed.iter().map(|detail| detail.run.id).collect();
    let newest: Vec<RunId> = started.iter().rev().take(50).copied().collect();
    assert_eq!(listed, newest);
}

/// An agent's question is put to a person on a Gate, which keeps what fits
/// on one: an empty question, or one too long to keep, is refused, so the
/// agent hears it must ask again.
#[tokio::test]
async fn a_question_must_fit_a_gate() {
    let repo = FakeRepo::default();
    let run_id = julia_s_run(&repo).await;
    let key = StepKey { run_id, seq: 1 };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    let session = StepSession {
        session_id: Uuid::now_v7(),
        action_id: Uuid::now_v7(),
        agent: None,
        bot: None,
    };
    let session_id = session.session_id;
    let planner = Role::try_from("planner".to_string()).expect("valid");
    repo.claim_session(
        key,
        session,
        &planner,
        &[Outcome::done(), Outcome::question()],
    )
    .await
    .expect("claim");
    let service = service(&repo, &FakeWorkflows::default());
    let asking = |output: String| HandBack {
        outcome: Outcome::question(),
        output,
    };

    let empty = service
        .hand_back(session_id, julia(), asking("  \n".to_string()))
        .await;
    let long = service
        .hand_back(
            session_id,
            julia(),
            asking("?".repeat(MAX_QUESTION_BYTES + 1)),
        )
        .await;
    service
        .hand_back(session_id, julia(), asking("CSV or JSON?".to_string()))
        .await
        .expect("a question that fits");

    assert!(matches!(empty, Err(FactoryError::QuestionSize { .. })));
    assert!(matches!(long, Err(FactoryError::QuestionSize { .. })));
}

/// An agent's question is answered in the owner's own words: an answer to
/// a question Gate with no note never reaches the workflow.
#[tokio::test]
async fn a_question_is_answered_in_words() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run_id = julia_s_run(&repo).await;
    let key = StepKey { run_id, seq: 1 };
    let entry = EntryId::try_from("plan".to_string()).expect("valid");
    repo.start_step(key, &entry, 1).await.expect("start");
    repo.end_step(key, &Outcome::question(), "CSV or JSON?")
        .await
        .expect("end");
    repo.open_gate(key, "CSV or JSON?", &[Outcome::answer()])
        .await
        .expect("open");
    let service = service(&repo, &workflows);

    let blank = service
        .answer_gate(
            julia(),
            run_id,
            1,
            Outcome::answer(),
            Some(" ".to_string()),
            None,
            Answerer::InPerson,
        )
        .await;
    let none = service
        .answer_gate(
            julia(),
            run_id,
            1,
            Outcome::answer(),
            None,
            None,
            Answerer::InPerson,
        )
        .await;
    service
        .answer_gate(
            julia(),
            run_id,
            1,
            Outcome::answer(),
            Some("CSV".to_string()),
            None,
            Answerer::InPerson,
        )
        .await
        .expect("answer");

    assert!(matches!(blank, Err(FactoryError::AnswerMissing)));
    assert!(matches!(none, Err(FactoryError::AnswerMissing)));
    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].note.as_deref(), Some("CSV"));
}

/// A Run started now lets its agents ask the service's questions, kept in
/// the Flow it starts with.
#[tokio::test]
async fn a_run_s_flow_keeps_how_many_questions_its_agents_may_ask() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();

    for (questions, expected) in [(None, DEFAULT_QUESTIONS), (Some(0), 0)] {
        let service = match questions {
            Some(questions) => service(&repo, &workflows).with_questions(questions),
            None => service(&repo, &workflows),
        };
        service
            .create_run(
                julia(),
                "reply".to_string(),
                "Summarise the release notes".to_string(),
                None,
                None,
                true,
            )
            .await
            .expect("create");
        let started = workflows.started.lock().expect("started").clone();
        assert_eq!(
            started.last().expect("started").flow.questions,
            Some(expected)
        );
    }
}
