//! The factory's use cases: starting and reading Runs, and running their
//! Steps.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use agent_session::domain::session_task::TaskDocumentId;
use chrono::Utc;
use factory_records::domain::main_hold::{BaseState, MainHoldRepository};
use factory_records::domain::models::{
    HandBack, MAX_NOTE_BYTES, MAX_OUTPUT_BYTES, MAX_REQUEST_BYTES, Outcome, Repository, Run, RunId,
    RunInsert, RunOutcome, Step, TaskLink,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::built_in_flows::{BUILT_IN_FLOWS, BuiltInFlow};
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening};
use crate::domain::models::{
    CodeRepository, FactoryError, FlowSummary, RunDetail, RunFinish, RunStart, StepEnding,
    StepError, StepInput, StepReport, StepRequest, StepStart,
};
use crate::domain::ports::{
    AnswerDelivery, FactoryService, PullRequests, Retryable, RunWorkflows, RunWorkflowsError,
    StepExecutor, StepService, TaskAccess, TaskDirectory, UserDirectory,
};

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> rootcause::Report {
    rootcause::Report::new(error).into_dynamic()
}

impl From<RunWorkflowsError> for FactoryError {
    fn from(error: RunWorkflowsError) -> Self {
        match error {
            RunWorkflowsError::Unavailable(upstream) => FactoryError::Upstream(upstream),
            RunWorkflowsError::Other(report) => FactoryError::Internal(report),
        }
    }
}

/// The most Runs a task's list holds: its owner's newest.
pub const TASK_RUNS_LISTED: u16 = 50;

/// How old a Run with no Step and no workflow must be before it counts as
/// one whose workflow never started.
const UNSTARTED_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

/// The Run API, over the Run records, the Runs' workflows, the worker that
/// says which Steps it serves, the people and tasks Runs name, and GitHub,
/// which says whether a Run's owner reaches its repository.
#[derive(Debug, Clone)]
pub struct FactoryServiceImpl<R, W, E, U, T, G> {
    repo: R,
    workflows: W,
    executor: E,
    users: U,
    tasks: T,
    github: G,
}

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository + MainHoldRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    /// Create the Run API.
    pub fn new(repo: R, workflows: W, executor: E, users: U, tasks: T, github: G) -> Self {
        Self {
            repo,
            workflows,
            executor,
            users,
            tasks,
            github,
        }
    }

    /// The repository `repo_url` names, with the branch its Run starts from,
    /// when `user_id` reaches it with their GitHub account and may push to
    /// it, as the Run's agents push its branch and the Run merges it.
    async fn reachable_repository(
        &self,
        user_id: &MacroUserIdStr<'static>,
        repo_url: &str,
    ) -> Result<Repository, FactoryError> {
        let repository = CodeRepository::parse(repo_url).ok_or(FactoryError::InvalidRepository)?;
        let access = self
            .github
            .repository_access(user_id, &repository)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::UnreachableRepository)?;
        if !access.can_push {
            return Err(FactoryError::NoPushAccess);
        }
        Ok(Repository {
            url: repository.url(),
            base_branch: access.default_branch,
        })
    }

    /// Fails unless `user_id` names an account.
    async fn known(&self, user_id: &MacroUserIdStr<'static>) -> Result<(), FactoryError> {
        if self.users.user_exists(user_id).await.map_err(internal)? {
            Ok(())
        } else {
            Err(FactoryError::UnknownUser)
        }
    }

    /// Fails unless the task document `task_id` is a task `user_id` can see.
    async fn visible_task(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Result<(), FactoryError> {
        let task =
            TaskDocumentId::parse(&task_id.to_string()).map_err(|_| FactoryError::UnknownTask)?;
        match self
            .tasks
            .task_access(user_id, &task)
            .await
            .map_err(internal)?
        {
            TaskAccess::Visible => Ok(()),
            TaskAccess::NotATask => Err(FactoryError::NotATask),
            TaskAccess::NotFound => Err(FactoryError::UnknownTask),
        }
    }

    /// `actor`'s Run. Someone else's Run reads as missing, so a Run id says
    /// nothing to anyone but its owner.
    async fn owned_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> Result<Run, FactoryError> {
        self.repo
            .get_run(id)
            .await
            .map_err(internal)?
            .filter(|run| run.user_id == *actor)
            .ok_or(FactoryError::NotFound)
    }

    /// Whether `steps`' latest Step is at a merge: its entry runs
    /// `ready_to_merge` or `merge` in a built-in Flow.
    fn at_merge(&self, steps: &[Step]) -> bool {
        let Some(latest) = steps.last() else {
            return false;
        };
        BUILT_IN_FLOWS
            .into_iter()
            .filter_map(|flow| flow.read(|step| self.executor.serves(step)).ok())
            .flat_map(|flow| flow.entries)
            .any(|entry| {
                entry.id == latest.entry
                    && matches!(entry.step.as_str(), "ready_to_merge" | "merge")
            })
    }

    /// What the Run shows of the base branch it merges to: nothing unless
    /// it goes and is at its merge, so only a Run about to merge shows a
    /// hold. `known` keeps what was read of each base, for a list of Runs.
    async fn base_shown(
        &self,
        run: &Run,
        steps: &[Step],
        known: &mut BTreeMap<(String, String), BaseState>,
    ) -> Result<BaseState, FactoryError> {
        let Some(repository) = run.repository.as_ref() else {
            return Ok(BaseState::default());
        };
        if run.end.is_some() || !self.at_merge(steps) {
            return Ok(BaseState::default());
        }
        let base = (repository.url.clone(), repository.base_branch.clone());
        if let Some(state) = known.get(&base) {
            return Ok(state.clone());
        }
        let state = self.repo.base_state(repository).await.map_err(internal)?;
        known.insert(base, state.clone());
        Ok(state)
    }

    /// `run` with its Steps, as the Run API shows it.
    async fn detail(&self, run: Run, steps: Vec<Step>) -> Result<RunDetail, FactoryError> {
        let base = self.base_shown(&run, &steps, &mut BTreeMap::new()).await?;
        Ok(RunDetail {
            run,
            steps,
            main_hold: base.hold,
            base_required_checks: base.required,
        })
    }

    /// Remove a Run whose workflow did not start, unless it did after all:
    /// a start that failed after the server took it leaves a workflow
    /// behind, which needs its Run.
    async fn forget_unstarted(&self, run_id: RunId) {
        match self.workflows.exists(run_id).await {
            Ok(false) => {
                if let Err(cleanup) = self.repo.delete_run(run_id).await {
                    tracing::error!(error = ?cleanup, "failed to remove a run whose workflow did not start");
                }
            }
            Ok(true) => {}
            Err(error) => {
                tracing::error!(
                    ?error,
                    "could not tell whether a run's workflow started; the run is kept"
                );
            }
        }
    }
}

impl<R, W, E, U, T, G> FactoryService for FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository + MainHoldRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    fn list_flows(&self) -> Vec<FlowSummary> {
        // A Flow that names a Step the worker does not serve yet is not
        // offered: a Run of it would be refused.
        BUILT_IN_FLOWS
            .into_iter()
            .filter_map(|flow| {
                let read = flow.read(|step| self.executor.serves(step)).ok()?;
                Some(FlowSummary {
                    name: flow.name.to_string(),
                    description: read.description,
                    repository: read.repository,
                })
            })
            .collect()
    }

    #[tracing::instrument(skip_all, fields(run_id = tracing::field::Empty))]
    async fn create_run(
        &self,
        actor: MacroUserIdStr<'static>,
        flow: String,
        request: String,
        task_id: Option<Uuid>,
        repo_url: Option<String>,
    ) -> Result<RunDetail, FactoryError> {
        let built_in = BuiltInFlow::named(&flow).ok_or(FactoryError::UnknownFlow)?;
        let flow = built_in.read(|step| self.executor.serves(step))?;
        if request.trim().is_empty() {
            return Err(FactoryError::EmptyRequest);
        }
        if request.len() > MAX_REQUEST_BYTES {
            return Err(FactoryError::RequestTooLarge {
                max: MAX_REQUEST_BYTES,
            });
        }
        match (flow.repository, &repo_url) {
            (true, None) => return Err(FactoryError::RepositoryRequired),
            (false, Some(_)) => return Err(FactoryError::RepositoryNotUsed),
            _ => {}
        }
        self.known(&actor).await?;
        if let Some(task_id) = task_id {
            self.visible_task(&actor, task_id).await?;
        }
        // The owner's GitHub account must reach the repository now: every
        // Step that works on it acts as them.
        let repository = match &repo_url {
            Some(repo_url) => Some(self.reachable_repository(&actor, repo_url).await?),
            None => None,
        };

        // The Run and its task are kept in one write, so a task deleted
        // since it was checked leaves nothing behind.
        let run_id = RunId::generate();
        tracing::Span::current().record("run_id", tracing::field::display(run_id));
        let run = match self
            .repo
            .insert_run(
                run_id,
                built_in.name,
                &request,
                &actor,
                task_id,
                repository.as_ref(),
            )
            .await
            .map_err(internal)?
        {
            RunInsert::Inserted(run) => run,
            RunInsert::NoSuchTask => return Err(FactoryError::UnknownTask),
        };

        // The Run is recorded before its workflow starts, so the workflow can
        // record its Steps against it.
        let start = RunStart { run_id, flow };
        if let Err(error) = self.workflows.start(&start).await {
            self.forget_unstarted(run_id).await;
            return Err(error.into());
        }

        self.detail(run, Vec::new()).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn get_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> Result<RunDetail, FactoryError> {
        let mut run = self.owned_run(actor, id).await?;
        let steps = self.repo.list_steps(id).await.map_err(internal)?;

        // A Run kept when its workflow's start could not be checked, and
        // whose workflow never came, would wait forever: once it is old
        // enough that a started workflow would have taken a Step, and
        // Temporal says it has no workflow, it ends failed.
        let stale = Utc::now() - run.created_at > UNSTARTED_AFTER;
        if run.end.is_none()
            && steps.is_empty()
            && stale
            && let Ok(false) = self.workflows.exists(id).await
        {
            self.repo
                .end_run(id, RunOutcome::Failed)
                .await
                .map_err(internal)?;
            run = self.owned_run(actor, id).await?;
        }
        self.detail(run, steps).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn list_task_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Result<Vec<RunDetail>, FactoryError> {
        // The path names a task: a document that is not one, one the
        // caller can't see, and a missing one all read as no such task.
        let task =
            TaskDocumentId::parse(&task_id.to_string()).map_err(|_| FactoryError::TaskNotFound)?;
        match self
            .tasks
            .task_access(actor, &task)
            .await
            .map_err(internal)?
        {
            TaskAccess::Visible => {}
            TaskAccess::NotATask | TaskAccess::NotFound => return Err(FactoryError::TaskNotFound),
        }

        let runs = self
            .repo
            .list_task_runs(task_id, actor, TASK_RUNS_LISTED)
            .await
            .map_err(internal)?;
        let ids: Vec<RunId> = runs.iter().map(|run| run.id).collect();
        let mut steps = self.repo.list_runs_steps(&ids).await.map_err(internal)?;
        // Runs on one base share what was read of it.
        let mut known = BTreeMap::new();
        let mut details = Vec::with_capacity(runs.len());
        for run in runs {
            let steps = steps.remove(&run.id).unwrap_or_default();
            let base = self.base_shown(&run, &steps, &mut known).await?;
            details.push(RunDetail {
                run,
                steps,
                main_hold: base.hold,
                base_required_checks: base.required,
            });
        }
        Ok(details)
    }

    #[tracing::instrument(skip(self))]
    async fn clear_main_hold(
        &self,
        repo_url: &str,
        base_branch: &str,
    ) -> Result<bool, FactoryError> {
        let repository = CodeRepository::parse(repo_url).ok_or(FactoryError::InvalidRepository)?;
        let base = Repository {
            url: repository.url(),
            base_branch: base_branch.to_string(),
        };
        let held = self
            .repo
            .main_hold(&base)
            .await
            .map_err(internal)?
            .is_some();
        self.repo
            .clear_main_hold(&base, Utc::now())
            .await
            .map_err(internal)?;
        tracing::info!(held, "an operator cleared a main hold");
        Ok(held)
    }

    #[tracing::instrument(skip(self, actor))]
    async fn link_task(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
        task_id: Uuid,
    ) -> Result<RunDetail, FactoryError> {
        self.owned_run(actor, id).await?;
        self.visible_task(actor, task_id).await?;

        match self.repo.link_task(id, task_id).await.map_err(internal)? {
            TaskLink::Linked => self.get_run(actor, id).await,
            TaskLink::NoSuchRun => Err(FactoryError::NotFound),
            TaskLink::NoSuchTask => Err(FactoryError::UnknownTask),
        }
    }

    #[tracing::instrument(skip(self, actor, answer, note), fields(answer = %answer))]
    async fn answer_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        id: RunId,
        seq: u16,
        answer: Outcome,
        note: Option<String>,
    ) -> Result<(), FactoryError> {
        let note_bytes = note.as_ref().map_or(0, String::len);
        if note_bytes > MAX_NOTE_BYTES {
            return Err(FactoryError::NoteTooLarge {
                max: MAX_NOTE_BYTES,
            });
        }
        self.known(&actor).await?;
        let run = self.owned_run(&actor, id).await?;
        if run.end.is_some() {
            return Err(FactoryError::RunEnded);
        }

        // An answer the recorded Gate does not offer is the caller's mistake;
        // which Gate the Run waits on right now only its workflow knows.
        let steps = self.repo.list_steps(id).await.map_err(internal)?;
        let gate = steps
            .iter()
            .find(|step| step.seq == seq)
            .and_then(|step| step.gate.as_ref());
        if let Some(gate) = gate
            && !gate.answers.contains(&answer)
        {
            return Err(FactoryError::AnswerNotOffered {
                offered: gate.answers.iter().map(|a| a.as_str().to_owned()).collect(),
            });
        }

        let answer = AnswerGate {
            seq,
            answer,
            note,
            user_id: actor,
        };
        match self.workflows.answer(id, &answer).await? {
            AnswerDelivery::Taken => Ok(()),
            AnswerDelivery::Refused(why) => Err(FactoryError::AnswerRefused(why)),
        }
    }

    #[tracing::instrument(skip(self, user_id, hand_back), fields(outcome = %hand_back.outcome))]
    async fn hand_back(
        &self,
        session_id: Uuid,
        user_id: MacroUserIdStr<'static>,
        hand_back: HandBack,
    ) -> Result<(), FactoryError> {
        if hand_back.output.len() > MAX_OUTPUT_BYTES {
            return Err(FactoryError::HandBackTooLarge {
                max: MAX_OUTPUT_BYTES,
            });
        }
        let step = self
            .repo
            .session_step(session_id)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NotAStepSession)?;

        // The session runs as the Run's owner, so its code-mode calls do too.
        if step.owner != user_id {
            return Err(FactoryError::NotTheOwner);
        }
        if step.ended {
            return Err(FactoryError::StepEnded);
        }
        if !step.offered.contains(&hand_back.outcome) {
            return Err(FactoryError::OutcomeNotOffered {
                offered: step.offered.iter().map(|o| o.as_str().to_owned()).collect(),
            });
        }
        // The Step can end between the read above and this write: a hand
        // back that lands after it is lost, and the agent must hear so.
        let kept = self
            .repo
            .hand_back(session_id, &hand_back)
            .await
            .map_err(internal)?;
        if kept {
            Ok(())
        } else {
            Err(FactoryError::StepEnded)
        }
    }

    #[tracing::instrument(skip(self, actor))]
    async fn cancel_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> Result<(), FactoryError> {
        let run = self.owned_run(actor, id).await?;
        match run.end.map(|end| end.outcome) {
            None => {
                if self.workflows.cancel(id).await? {
                    return Ok(());
                }
                // Temporal had no running workflow for the Run. One may have
                // started since, which a second cancel reaches; one that
                // completed before its Run's end was recorded has ended the
                // Run, though the records don't say so yet.
                if self.workflows.exists(id).await? {
                    return if self.workflows.cancel(id).await? {
                        Ok(())
                    } else {
                        Err(FactoryError::RunEnded)
                    };
                }
                // A Run whose workflow never started has nothing to stop: it
                // ends cancelled here. A workflow that starts late takes no
                // Step of it, and is cancelled if it is there by now.
                self.repo
                    .end_run(id, RunOutcome::Cancelled)
                    .await
                    .map_err(internal)?;
                if self.workflows.exists(id).await? {
                    self.workflows.cancel(id).await?;
                }
                Ok(())
            }
            // Cancelling again is what the first cancel already did.
            Some(RunOutcome::Cancelled) => Ok(()),
            Some(_) => Err(FactoryError::RunEnded),
        }
    }
}

/// What a Run's worker does for its workflow: run Steps, and record Steps,
/// Gates and the Run's end, each on its own.
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

    async fn run_of(&self, request: &StepRequest) -> Result<Run, StepError> {
        self.repo
            .get_run(request.key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)
    }
}

impl<R, E> StepService for StepServiceImpl<R, E>
where
    R: RunRepository,
    E: StepExecutor,
{
    #[tracing::instrument(err, skip_all, fields(run_id = %request.key.run_id, seq = request.key.seq))]
    async fn start_step(&self, request: StepRequest) -> Result<StepStart, StepError> {
        // A Run cancelled before its workflow started takes no Step.
        if let Some(end) = self.run_of(&request).await?.end {
            return Ok(StepStart::RunEnded(end.outcome));
        }
        self.repo
            .start_step(request.key, &request.entry, request.visit)
            .await
            .map_err(internal)?;
        Ok(StepStart::Started)
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %request.key.run_id, seq = request.key.seq))]
    async fn run_step(&self, request: StepRequest) -> Result<StepReport, StepError> {
        let run = self.run_of(&request).await?;
        // A Run that ended does no more work, whichever attempt asks.
        if run.end.is_some() {
            return Err(StepError::RunEnded);
        }

        // The Step reads what the Run's Steps before it produced.
        let earlier: Vec<_> = self
            .repo
            .list_steps(request.key.run_id)
            .await
            .map_err(internal)?
            .into_iter()
            .filter(|step| step.seq < request.key.seq && step.end.is_some())
            .collect();
        let input = StepInput {
            key: request.key,
            role: request.role.as_ref(),
            owner: &run.user_id,
            request: &run.request,
            task_id: run.task_id,
            repository: run.repository.as_ref(),
            earlier: &earlier,
        };
        let report = self
            .executor
            .run(&request.step, input)
            .await
            .map_err(|error| {
                let final_failure = !error.retryable();
                let report = internal(error);
                if final_failure {
                    StepError::Final(report)
                } else {
                    StepError::Internal(report)
                }
            })?;
        if report.output.len() > MAX_OUTPUT_BYTES {
            return Err(StepError::OutputTooLarge {
                max: MAX_OUTPUT_BYTES,
            });
        }
        Ok(report)
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %request.key.run_id, seq = request.key.seq))]
    async fn stop_step(&self, request: StepRequest) -> Result<(), StepError> {
        let run = self.run_of(&request).await?;
        self.executor
            .stop(
                &request.step,
                request.role.as_ref(),
                request.key,
                &run.user_id,
            )
            .await
            .map_err(internal)?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %request.key.run_id, seq = request.key.seq))]
    async fn release_step(&self, request: StepRequest) -> Result<(), StepError> {
        let run = self.run_of(&request).await?;
        self.executor
            .release(
                &request.step,
                request.role.as_ref(),
                request.key,
                &run.user_id,
            )
            .await
            .map_err(internal)?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %ending.key.run_id, seq = ending.key.seq, outcome = %ending.report.outcome))]
    async fn end_step(&self, ending: StepEnding) -> Result<(), StepError> {
        self.repo
            .end_step(ending.key, &ending.report.outcome, &ending.report.output)
            .await
            .map_err(internal)?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %opening.key.run_id, seq = opening.key.seq))]
    async fn open_gate(&self, opening: GateOpening) -> Result<(), StepError> {
        self.repo
            .open_gate(opening.key, &opening.question, &opening.answers)
            .await
            .map_err(internal)?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %answered.key.run_id, seq = answered.key.seq))]
    async fn answer_gate(&self, answered: GateAnswered) -> Result<(), StepError> {
        let answer = &answered.answer;
        self.repo
            .answer_gate(
                answered.key,
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
