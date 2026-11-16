use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use factory_records::domain::models::{
    EntryId, Outcome, Run, RunEnd, RunId, RunOutcome, Step, StepEnd,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;

use super::{FactoryServiceImpl, StepServiceImpl};
use crate::domain::flow::StepName;
use crate::domain::gate::AnswerGate;
use crate::domain::models::{
    FactoryError, RunFinish, RunStart, StepInput, StepReport, StepRequest,
};
use crate::domain::ports::{
    AnswerDelivery, FactoryService, RunWorkflows, StepExecutor, StepService,
};

#[derive(Debug, thiserror::Error)]
#[error("fake failure")]
struct FakeError;

#[derive(Clone, Default)]
struct FakeRepo {
    runs: Arc<Mutex<BTreeMap<RunId, Run>>>,
    steps: Arc<Mutex<Vec<(RunId, Step)>>>,
}

impl RunRepository for FakeRepo {
    type Err = FakeError;

    async fn insert_run(&self, id: RunId, request: &str) -> Result<Run, FakeError> {
        let run = Run {
            id,
            request: request.to_string(),
            created_at: Utc::now(),
            end: None,
        };
        self.runs.lock().expect("runs").insert(id, run.clone());
        Ok(run)
    }

    async fn delete_run(&self, id: RunId) -> Result<(), FakeError> {
        self.runs.lock().expect("runs").remove(&id);
        Ok(())
    }

    async fn get_run(&self, id: RunId) -> Result<Option<Run>, FakeError> {
        Ok(self.runs.lock().expect("runs").get(&id).cloned())
    }

    async fn list_steps(&self, id: RunId) -> Result<Vec<Step>, FakeError> {
        let steps = self.steps.lock().expect("steps");
        Ok(steps
            .iter()
            .filter(|(run, _)| *run == id)
            .map(|(_, step)| step.clone())
            .collect())
    }

    async fn start_step(&self, id: RunId, entry: &EntryId, visit: u16) -> Result<(), FakeError> {
        self.steps.lock().expect("steps").push((
            id,
            Step {
                entry: entry.clone(),
                visit,
                started_at: Utc::now(),
                end: None,
                gate: None,
            },
        ));
        Ok(())
    }

    async fn end_step(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
        outcome: &Outcome,
        output: &str,
    ) -> Result<(), FakeError> {
        let mut steps = self.steps.lock().expect("steps");
        for (run, step) in steps.iter_mut() {
            if *run == id && step.entry == *entry && step.visit == visit {
                step.end = Some(StepEnd {
                    outcome: outcome.clone(),
                    output: output.to_string(),
                    ended_at: Utc::now(),
                });
            }
        }
        Ok(())
    }

    async fn open_gate(
        &self,
        _id: RunId,
        _entry: &EntryId,
        _visit: u16,
        _question: &str,
        _answers: &[Outcome],
    ) -> Result<(), FakeError> {
        Ok(())
    }

    async fn answer_gate(
        &self,
        _id: RunId,
        _entry: &EntryId,
        _visit: u16,
        _answer: &Outcome,
        _note: Option<&str>,
        _user_id: &MacroUserIdStr<'static>,
    ) -> Result<(), FakeError> {
        Ok(())
    }

    async fn end_run(&self, id: RunId, outcome: RunOutcome) -> Result<(), FakeError> {
        if let Some(run) = self.runs.lock().expect("runs").get_mut(&id) {
            run.end = Some(RunEnd {
                outcome,
                ended_at: Utc::now(),
            });
        }
        Ok(())
    }
}

/// Keeps what it was sent. Refuses every answer when `refuse` is set.
#[derive(Clone, Default)]
struct FakeWorkflows {
    started: Arc<Mutex<Vec<RunStart>>>,
    answers: Arc<Mutex<Vec<AnswerGate>>>,
    cancelled: Arc<Mutex<Vec<RunId>>>,
    fail: bool,
    refuse: bool,
}

impl RunWorkflows for FakeWorkflows {
    type Err = FakeError;

    async fn start(&self, start: &RunStart) -> Result<(), FakeError> {
        if self.fail {
            return Err(FakeError);
        }
        self.started.lock().expect("started").push(start.clone());
        Ok(())
    }

    async fn answer(
        &self,
        _run_id: RunId,
        answer: &AnswerGate,
    ) -> Result<AnswerDelivery, FakeError> {
        if self.refuse {
            return Ok(AnswerDelivery::Refused("not waiting".to_string()));
        }
        self.answers.lock().expect("answers").push(answer.clone());
        Ok(AnswerDelivery::Taken)
    }

    async fn cancel(&self, run_id: RunId) -> Result<(), FakeError> {
        self.cancelled.lock().expect("cancelled").push(run_id);
        Ok(())
    }
}

/// Serves `reply`, which ends `done` with a fixed output, and keeps the
/// entries of the earlier Steps each run was given.
#[derive(Clone)]
struct FakeExecutor {
    output: String,
    given: Arc<Mutex<Vec<Vec<String>>>>,
}

impl Default for FakeExecutor {
    fn default() -> Self {
        Self {
            output: "Replied".to_string(),
            given: Arc::default(),
        }
    }
}

impl StepExecutor for FakeExecutor {
    type Err = FakeError;

    fn serves(&self, step: &StepName) -> bool {
        step.as_str() == "reply"
    }

    async fn run(&self, _step: &StepName, input: StepInput<'_>) -> Result<StepReport, FakeError> {
        let earlier = input
            .earlier
            .iter()
            .map(|step| format!("{}#{}", step.entry, step.visit))
            .collect();
        self.given.lock().expect("given").push(earlier);
        Ok(StepReport {
            outcome: Outcome::try_from("done".to_string()).expect("valid"),
            output: self.output.clone(),
        })
    }
}

fn reply_flow() -> serde_json::Value {
    json!({
        "entries": [{
            "id": "reply",
            "step": "reply",
            "on": { "done": [{ "to": { "kind": "end", "outcome": "success" } }] }
        }]
    })
}

fn reply_step(run_id: RunId) -> StepRequest {
    StepRequest {
        run_id,
        entry: EntryId::try_from("reply".to_string()).expect("valid"),
        step: StepName::try_from("reply".to_string()).expect("valid"),
        visit: 1,
        request: "Summarise the release notes".to_string(),
    }
}

#[tokio::test]
async fn creating_a_run_records_it_and_starts_its_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = FactoryServiceImpl::new(repo.clone(), workflows.clone(), FakeExecutor::default());

    let created = service
        .create_run(reply_flow(), "Summarise the release notes".to_string())
        .await
        .expect("create");

    let started = workflows.started.lock().expect("started").clone();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].run_id, created.run.id);
    assert_eq!(started[0].request, "Summarise the release notes");
    let read = service.get_run(created.run.id).await.expect("get");
    assert_eq!(read.run, created.run);
    assert_eq!(read.run.end, None);
}

#[tokio::test]
async fn a_run_whose_workflow_does_not_start_is_not_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows {
        fail: true,
        ..FakeWorkflows::default()
    };
    let service = FactoryServiceImpl::new(repo.clone(), workflows, FakeExecutor::default());

    let refused = service
        .create_run(reply_flow(), "Summarise the release notes".to_string())
        .await;

    assert!(matches!(refused, Err(FactoryError::Internal(_))));
    assert!(repo.runs.lock().expect("runs").is_empty());
}

#[tokio::test]
async fn a_flow_with_a_step_no_worker_serves_is_refused_before_anything_is_kept() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let service = FactoryServiceImpl::new(repo.clone(), workflows.clone(), FakeExecutor::default());
    let flow = json!({ "entries": [{ "id": "plan", "step": "plan" }] });

    let refused = service
        .create_run(flow, "Plan the migration".to_string())
        .await;

    assert!(matches!(refused, Err(FactoryError::InvalidFlow(_))));
    assert!(repo.runs.lock().expect("runs").is_empty());
    assert!(workflows.started.lock().expect("started").is_empty());
}

#[tokio::test]
async fn an_empty_request_is_refused() {
    let service = FactoryServiceImpl::new(
        FakeRepo::default(),
        FakeWorkflows::default(),
        FakeExecutor::default(),
    );

    let refused = service.create_run(reply_flow(), "  ".to_string()).await;

    assert!(matches!(refused, Err(FactoryError::EmptyRequest)));
}

#[tokio::test]
async fn reading_an_unknown_run_is_not_found() {
    let service = FactoryServiceImpl::new(
        FakeRepo::default(),
        FakeWorkflows::default(),
        FakeExecutor::default(),
    );

    let missing = service.get_run(RunId::generate()).await;

    assert!(matches!(missing, Err(FactoryError::NotFound)));
}

#[tokio::test]
async fn running_a_step_records_its_start_outcome_and_output() {
    let repo = FakeRepo::default();
    let run = repo
        .insert_run(RunId::generate(), "Summarise")
        .await
        .expect("insert");
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());

    let report = steps.run_step(reply_step(run.id)).await.expect("run");

    assert_eq!(report.outcome.as_str(), "done");
    let recorded = repo.list_steps(run.id).await.expect("steps");
    assert_eq!(recorded.len(), 1);
    let end = recorded[0].end.as_ref().expect("the step ended");
    assert_eq!(end.outcome.as_str(), "done");
    assert_eq!(end.output, "Replied");
}

#[tokio::test]
async fn a_step_is_given_the_steps_that_ended_before_it() {
    let repo = FakeRepo::default();
    let run = repo
        .insert_run(RunId::generate(), "Summarise")
        .await
        .expect("insert");
    let executor = FakeExecutor::default();
    let steps = StepServiceImpl::new(repo.clone(), executor.clone());

    steps.run_step(reply_step(run.id)).await.expect("first");
    steps
        .run_step(StepRequest {
            visit: 2,
            ..reply_step(run.id)
        })
        .await
        .expect("second");

    let given = executor.given.lock().expect("given").clone();
    assert_eq!(given, [Vec::<String>::new(), vec!["reply#1".to_string()]]);
}

#[tokio::test]
async fn a_step_whose_output_is_too_large_is_left_unended() {
    let repo = FakeRepo::default();
    let run = repo
        .insert_run(RunId::generate(), "Summarise")
        .await
        .expect("insert");
    let executor = FakeExecutor {
        output: "x".repeat(65_537),
        ..FakeExecutor::default()
    };
    let steps = StepServiceImpl::new(repo.clone(), executor);

    let refused = steps.run_step(reply_step(run.id)).await;

    assert!(refused.is_err());
    let recorded = repo.list_steps(run.id).await.expect("steps");
    assert_eq!(recorded[0].end, None);
}

#[tokio::test]
async fn ending_a_run_records_its_outcome() {
    let repo = FakeRepo::default();
    let run = repo
        .insert_run(RunId::generate(), "Summarise")
        .await
        .expect("insert");
    let steps = StepServiceImpl::new(repo.clone(), FakeExecutor::default());

    steps
        .end_run(RunFinish {
            run_id: run.id,
            outcome: RunOutcome::Success,
        })
        .await
        .expect("end");

    let ended = repo.get_run(run.id).await.expect("get").expect("exists");
    assert_eq!(ended.end.map(|end| end.outcome), Some(RunOutcome::Success));
}

fn approval(note: Option<String>) -> AnswerGate {
    AnswerGate {
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        visit: 1,
        answer: Outcome::try_from("approve".to_string()).expect("valid"),
        note,
        user_id: MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid"),
    }
}

#[tokio::test]
async fn an_answer_is_sent_to_the_run_s_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run = repo
        .insert_run(RunId::generate(), "Plan")
        .await
        .expect("insert");
    let service = FactoryServiceImpl::new(repo, workflows.clone(), FakeExecutor::default());

    service
        .answer_gate(run.id, approval(Some("Go ahead".to_string())))
        .await
        .expect("answer");

    let answers = workflows.answers.lock().expect("answers").clone();
    assert_eq!(answers, [approval(Some("Go ahead".to_string()))]);
}

#[tokio::test]
async fn an_answer_the_workflow_refuses_is_a_conflict() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows {
        refuse: true,
        ..FakeWorkflows::default()
    };
    let run = repo
        .insert_run(RunId::generate(), "Plan")
        .await
        .expect("insert");
    let service = FactoryServiceImpl::new(repo, workflows, FakeExecutor::default());

    let refused = service.answer_gate(run.id, approval(None)).await;

    assert!(matches!(refused, Err(FactoryError::AnswerRefused(_))));
}

#[tokio::test]
async fn a_run_that_ended_takes_no_answer_and_no_cancel() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run = repo
        .insert_run(RunId::generate(), "Plan")
        .await
        .expect("insert");
    repo.end_run(run.id, RunOutcome::Success)
        .await
        .expect("end");
    let service = FactoryServiceImpl::new(repo, workflows.clone(), FakeExecutor::default());

    let answered = service.answer_gate(run.id, approval(None)).await;
    let cancelled = service.cancel_run(run.id).await;

    assert!(matches!(answered, Err(FactoryError::RunEnded)));
    assert!(matches!(cancelled, Err(FactoryError::RunEnded)));
    assert!(workflows.answers.lock().expect("answers").is_empty());
    assert!(workflows.cancelled.lock().expect("cancelled").is_empty());
}

#[tokio::test]
async fn a_note_larger_than_a_gate_keeps_is_refused() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run = repo
        .insert_run(RunId::generate(), "Plan")
        .await
        .expect("insert");
    let service = FactoryServiceImpl::new(repo, workflows.clone(), FakeExecutor::default());

    let refused = service
        .answer_gate(run.id, approval(Some("x".repeat(4_097))))
        .await;

    assert!(matches!(refused, Err(FactoryError::NoteTooLarge { .. })));
    assert!(workflows.answers.lock().expect("answers").is_empty());
}

#[tokio::test]
async fn cancelling_a_running_run_cancels_its_workflow() {
    let repo = FakeRepo::default();
    let workflows = FakeWorkflows::default();
    let run = repo
        .insert_run(RunId::generate(), "Plan")
        .await
        .expect("insert");
    let service = FactoryServiceImpl::new(repo, workflows.clone(), FakeExecutor::default());

    service.cancel_run(run.id).await.expect("cancel");

    assert_eq!(*workflows.cancelled.lock().expect("cancelled"), [run.id]);
}
