//! The factory's use cases: starting and reading Runs, and running their
//! Steps.

#[cfg(test)]
mod test;

use factory_records::domain::models::{
    MAX_NOTE_BYTES, MAX_OUTPUT_BYTES, MAX_REQUEST_BYTES, RunId, TaskLink,
};
use factory_records::domain::ports::{RunRepository, UserDirectory};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::flow::Flow;
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening};
use crate::domain::models::{
    FactoryError, RunDetail, RunFinish, RunStart, StepError, StepInput, StepReport, StepRequest,
};
use crate::domain::ports::{
    AnswerDelivery, FactoryService, RunWorkflows, StepExecutor, StepService, TaskAccess,
    TaskDirectory,
};

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> rootcause::Report {
    rootcause::Report::new(error).into_dynamic()
}

/// The Run API, over the Run records, the Runs' workflows, the worker that
/// says which Steps it serves, and the people and tasks Runs name.
#[derive(Debug, Clone)]
pub struct FactoryServiceImpl<R, W, E, U, T> {
    repo: R,
    workflows: W,
    executor: E,
    users: U,
    tasks: T,
}

impl<R, W, E, U, T> FactoryServiceImpl<R, W, E, U, T>
where
    R: RunRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
{
    /// Create the Run API.
    pub fn new(repo: R, workflows: W, executor: E, users: U, tasks: T) -> Self {
        Self {
            repo,
            workflows,
            executor,
            users,
            tasks,
        }
    }

    /// Fails unless `user_id` names an account.
    async fn known(&self, user_id: &MacroUserIdStr<'static>) -> Result<(), FactoryError> {
        if self.users.user_exists(user_id).await.map_err(internal)? {
            Ok(())
        } else {
            Err(FactoryError::UnknownUser)
        }
    }

    /// Fails unless `task_id` is a task `user_id` can see.
    async fn visible_task(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Result<(), FactoryError> {
        match self
            .tasks
            .task_access(user_id, task_id)
            .await
            .map_err(internal)?
        {
            TaskAccess::Visible => Ok(()),
            TaskAccess::NotATask => Err(FactoryError::NotATask),
            TaskAccess::NotFound => Err(FactoryError::UnknownTask),
        }
    }

    /// Fails unless the Run exists and has not ended.
    async fn running(&self, id: RunId) -> Result<(), FactoryError> {
        let run = self
            .repo
            .get_run(id)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NotFound)?;
        match run.end {
            Some(_) => Err(FactoryError::RunEnded),
            None => Ok(()),
        }
    }
}

impl<R, W, E, U, T> FactoryService for FactoryServiceImpl<R, W, E, U, T>
where
    R: RunRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
{
    #[tracing::instrument(err, skip_all, fields(run_id = tracing::field::Empty))]
    async fn create_run(
        &self,
        definition: serde_json::Value,
        request: String,
        user_id: MacroUserIdStr<'static>,
        task_id: Option<Uuid>,
    ) -> Result<RunDetail, FactoryError> {
        let flow = Flow::parse(definition, |step| self.executor.serves(step))?;
        if request.trim().is_empty() {
            return Err(FactoryError::EmptyRequest);
        }
        if request.len() > MAX_REQUEST_BYTES {
            return Err(FactoryError::RequestTooLarge {
                max: MAX_REQUEST_BYTES,
            });
        }

        self.known(&user_id).await?;
        if let Some(task_id) = task_id {
            self.visible_task(&user_id, task_id).await?;
        }

        let run_id = RunId::generate();
        tracing::Span::current().record("run_id", tracing::field::display(run_id));
        let mut run = self
            .repo
            .insert_run(run_id, &request, &user_id)
            .await
            .map_err(internal)?;

        // A task deleted since it was checked leaves no Run behind.
        if let Some(task_id) = task_id {
            let linked = self.repo.link_task(run_id, task_id).await;
            if !matches!(linked, Ok(TaskLink::Linked)) {
                if let Err(cleanup) = self.repo.delete_run(run_id).await {
                    tracing::error!(error = ?cleanup, "failed to remove a run whose task did not link");
                }
                return match linked {
                    Err(error) => Err(internal(error).into()),
                    Ok(_) => Err(FactoryError::UnknownTask),
                };
            }
            run.task_id = Some(task_id);
        }

        // The Run is recorded before its workflow starts, so the workflow can
        // record its Steps against it. A workflow that does not start leaves
        // no Run behind.
        let start = RunStart {
            run_id,
            flow,
            request,
        };
        if let Err(error) = self.workflows.start(&start).await {
            if let Err(cleanup) = self.repo.delete_run(run_id).await {
                tracing::error!(error = ?cleanup, "failed to remove a run whose workflow did not start");
            }
            return Err(internal(error).into());
        }

        Ok(RunDetail {
            run,
            steps: Vec::new(),
        })
    }

    #[tracing::instrument(err, skip(self))]
    async fn get_run(&self, id: RunId) -> Result<RunDetail, FactoryError> {
        let run = self
            .repo
            .get_run(id)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NotFound)?;
        let steps = self.repo.list_steps(id).await.map_err(internal)?;
        Ok(RunDetail { run, steps })
    }

    #[tracing::instrument(err, skip(self))]
    async fn link_task(&self, id: RunId, task_id: Uuid) -> Result<RunDetail, FactoryError> {
        let run = self
            .repo
            .get_run(id)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NotFound)?;
        self.visible_task(&run.user_id, task_id).await?;

        match self.repo.link_task(id, task_id).await.map_err(internal)? {
            TaskLink::Linked => self.get_run(id).await,
            TaskLink::NoSuchRun => Err(FactoryError::NotFound),
            TaskLink::NoSuchTask => Err(FactoryError::UnknownTask),
        }
    }

    #[tracing::instrument(err, skip(self, answer), fields(entry = %answer.entry, visit = answer.visit))]
    async fn answer_gate(&self, id: RunId, answer: AnswerGate) -> Result<(), FactoryError> {
        let note_bytes = answer.note.as_ref().map_or(0, String::len);
        if note_bytes > MAX_NOTE_BYTES {
            return Err(FactoryError::NoteTooLarge {
                max: MAX_NOTE_BYTES,
            });
        }
        self.known(&answer.user_id).await?;
        self.running(id).await?;

        // The workflow decides whether it takes the answer: only it knows
        // which Gate it waits on right now.
        match self.workflows.answer(id, &answer).await.map_err(internal)? {
            AnswerDelivery::Taken => Ok(()),
            AnswerDelivery::Refused(why) => Err(FactoryError::AnswerRefused(why)),
        }
    }

    #[tracing::instrument(err, skip(self))]
    async fn cancel_run(&self, id: RunId) -> Result<(), FactoryError> {
        self.running(id).await?;
        self.workflows.cancel(id).await.map_err(internal)?;
        Ok(())
    }
}

/// What a Run's worker does for its workflow: run and record Steps, and
/// record the Run's end.
#[derive(Debug, Clone)]
pub struct StepServiceImpl<R, E> {
    repo: R,
    executor: E,
}

impl<R, E> StepServiceImpl<R, E>
where
    R: RunRepository,
    E: StepExecutor,
{
    /// Create the Step service.
    pub fn new(repo: R, executor: E) -> Self {
        Self { repo, executor }
    }
}

impl<R, E> StepService for StepServiceImpl<R, E>
where
    R: RunRepository,
    E: StepExecutor,
{
    #[tracing::instrument(
        err,
        skip_all,
        fields(run_id = %request.run_id, entry = %request.entry, visit = request.visit)
    )]
    async fn run_step(&self, request: StepRequest) -> Result<StepReport, StepError> {
        self.repo
            .start_step(request.run_id, &request.entry, request.visit)
            .await
            .map_err(internal)?;

        // The Step reads what the Run's Steps before it produced.
        let earlier: Vec<_> = self
            .repo
            .list_steps(request.run_id)
            .await
            .map_err(internal)?
            .into_iter()
            .filter(|step| step.end.is_some())
            .collect();
        let input = StepInput {
            run_id: request.run_id,
            request: &request.request,
            earlier: &earlier,
        };
        let report = self
            .executor
            .run(&request.step, input)
            .await
            .map_err(internal)?;
        if report.output.len() > MAX_OUTPUT_BYTES {
            return Err(StepError::OutputTooLarge {
                max: MAX_OUTPUT_BYTES,
            });
        }

        self.repo
            .end_step(
                request.run_id,
                &request.entry,
                request.visit,
                &report.outcome,
                &report.output,
            )
            .await
            .map_err(internal)?;
        Ok(report)
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %opening.run_id, entry = %opening.entry))]
    async fn open_gate(&self, opening: GateOpening) -> Result<(), StepError> {
        self.repo
            .open_gate(
                opening.run_id,
                &opening.entry,
                opening.visit,
                &opening.question,
                &opening.answers,
            )
            .await
            .map_err(internal)?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %answered.run_id, entry = %answered.answer.entry))]
    async fn answer_gate(&self, answered: GateAnswered) -> Result<(), StepError> {
        let answer = &answered.answer;
        self.repo
            .answer_gate(
                answered.run_id,
                &answer.entry,
                answer.visit,
                &answer.answer,
                answer.note.as_deref(),
                &answer.user_id,
            )
            .await
            .map_err(internal)?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %finish.run_id, outcome = %finish.outcome))]
    async fn end_run(&self, finish: RunFinish) -> Result<(), StepError> {
        self.repo
            .end_run(finish.run_id, finish.outcome)
            .await
            .map_err(internal)?;
        Ok(())
    }
}
