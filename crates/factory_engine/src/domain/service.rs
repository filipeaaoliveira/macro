//! The factory's use cases: starting and reading Runs, and running their
//! Steps.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;
use std::num::NonZeroU8;

use agent_session::domain::session_task::TaskDocumentId;
use chrono::Utc;
use factory_records::domain::main_hold::{BaseState, MainHoldRepository};
use factory_records::domain::models::{
    HandBack, MAX_NOTE_BYTES, MAX_OUTPUT_BYTES, MAX_QUESTION_BYTES, MAX_REQUEST_BYTES, NewRun,
    Outcome, Repository, Run, RunId, RunInsert, RunOutcome, Step, TaskLink, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{ReviewRound, ReviewRoundRepository};
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, ProjectRulesSet, RuleGateKind, RulesSource, RunRules,
    StandingRulesRepository,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::built_in_flows::{BUILT_IN_FLOWS, BuiltInFlow};
use crate::domain::control_files::ControlFiles;
use crate::domain::gate::{AnswerGate, GateAnswered, GateOpening, GateRuling};
use crate::domain::models::{
    CodeRepository, ControlFilesSettings, FactoryError, FlowSummary, RunDetail, RunFinish,
    RunStart, StandingRulesSettings, StepEnding, StepError, StepInput, StepReport, StepRequest,
    StepStart,
};
use crate::domain::ports::{
    AnswerDelivery, FactoryService, PullRequests, Retryable, RunWorkflows, RunWorkflowsError,
    StepExecutor, StepService, TaskAccess, TaskDirectory, UserDirectory,
};
use crate::domain::standing_rules::{GateFacts, RuleAnswer, rule_answer};

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

/// The answer that agrees to what a Gate asks, which on a Tampered Run
/// approves the Control Files its branch changed.
const APPROVE: &str = "approve";

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
    review_rounds: Option<NonZeroU8>,
    questions: u8,
}

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository + MainHoldRepository + ReviewRoundRepository + StandingRulesRepository,
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
            review_rounds: None,
            questions: DEFAULT_QUESTIONS,
        }
    }

    /// Let each agent Step of the Runs it starts ask the Run's owner up to
    /// `questions` questions, in place of [`DEFAULT_QUESTIONS`]. The bound
    /// is kept in the Flow each Run starts with, as the review rounds'.
    pub fn with_questions(mut self, questions: u8) -> Self {
        self.questions = questions;
        self
    }

    /// Bound each review panel of the Runs it starts to `rounds` rounds,
    /// in place of the `max_visits` its Flow gives it. The bound is kept in
    /// the Flow each Run starts with, so a Run keeps it whatever the
    /// service is told later.
    pub fn with_review_rounds(mut self, rounds: NonZeroU8) -> Self {
        self.review_rounds = Some(rounds);
        self
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

    /// Fails when `user_id`'s account is an agent's: it signs in as a person
    /// does, yet never acts as one in person.
    async fn not_an_agent(&self, user_id: &MacroUserIdStr<'static>) -> Result<(), FactoryError> {
        if self
            .repo
            .is_agent_account(user_id)
            .await
            .map_err(internal)?
        {
            return Err(FactoryError::NotAPerson);
        }
        Ok(())
    }

    /// The team `user_id` is on, and the Control File patterns it set; none
    /// for either when it has none.
    async fn team_control_files(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<(Option<TeamMembership>, Option<Vec<String>>), FactoryError> {
        let team = self.repo.team_of(user_id).await.map_err(internal)?;
        let own = match team {
            Some(team) => self
                .repo
                .team_control_files(team.team_id)
                .await
                .map_err(internal)?,
            None => None,
        };
        Ok((team, own))
    }

    /// The Standing Rules a Run of `user_id`'s on the task `task_id` is
    /// created with: the Project's own, when its owner's team set rules
    /// for the Project the task is in, else the team's, else the
    /// factory's default.
    async fn run_rules(
        &self,
        user_id: &MacroUserIdStr<'static>,
        team: Option<TeamMembership>,
        task_id: Option<Uuid>,
    ) -> Result<RunRules, FactoryError> {
        let Some(team) = team else {
            return Ok(RunRules::DEFAULT);
        };
        if let Some(task_id) = task_id {
            // A Project that cannot be read now leaves the Run the team's
            // rules, rather than refusing the Run.
            let project = match self.tasks.task_project(user_id, task_id).await {
                Ok(project) => project,
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        %task_id,
                        "the run task's project could not be read: the run takes its team's standing rules"
                    );
                    None
                }
            };
            if let Some(project_id) = project
                && let Some(definition) = self
                    .repo
                    .project_rules(team.team_id, project_id)
                    .await
                    .map_err(internal)?
            {
                return Ok(RunRules {
                    definition,
                    source: RulesSource::Project,
                    project_id: Some(project_id),
                });
            }
        }
        let own = self.repo.team_rules(team.team_id).await.map_err(internal)?;
        Ok(match own {
            Some(definition) => RunRules {
                definition,
                source: RulesSource::Team,
                project_id: None,
            },
            None => RunRules::DEFAULT,
        })
    }

    /// The team `actor` administers, in an account that is not an
    /// agent's.
    async fn administered_team(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<TeamMembership, FactoryError> {
        self.not_an_agent(actor).await?;
        self.repo
            .team_of(actor)
            .await
            .map_err(internal)?
            .filter(|team| team.admin)
            .ok_or(FactoryError::NotATeamAdmin)
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
    async fn detail(
        &self,
        run: Run,
        steps: Vec<Step>,
        review_rounds: Vec<ReviewRound>,
    ) -> Result<RunDetail, FactoryError> {
        let base = self.base_shown(&run, &steps, &mut BTreeMap::new()).await?;
        Ok(RunDetail {
            run,
            steps,
            main_hold: base.hold,
            base_required_checks: base.required,
            review_rounds,
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
    R: RunRepository + MainHoldRepository + ReviewRoundRepository + StandingRulesRepository,
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
        in_person: bool,
    ) -> Result<RunDetail, FactoryError> {
        let built_in = BuiltInFlow::named(&flow).ok_or(FactoryError::UnknownFlow)?;
        let mut flow = built_in.read(|step| self.executor.serves(step))?;
        if let Some(rounds) = self.review_rounds {
            flow.bound_review_rounds(rounds);
        }
        flow.allow_questions(self.questions);
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

        // The Run keeps the Control File patterns it is checked against as
        // its owner's team has them now, so a change to the team's list,
        // whoever makes it, does not change a Run under way. An agent's
        // account is never its owner in person.
        let (team, own) = self.team_control_files(&actor).await?;
        let control_files_team_id = team.filter(|_| own.is_some()).map(|team| team.team_id);
        let control_file_patterns = own.unwrap_or_else(ControlFiles::default_patterns);
        let created_in_person =
            in_person && !self.repo.is_agent_account(&actor).await.map_err(internal)?;
        // The Run keeps its Standing Rules the same way: a change to the
        // team's or the Project's rules, or a task linked later, does not
        // change which of its Gates a rule answers.
        let standing_rules = self.run_rules(&actor, team, task_id).await?;

        // The Run and its task are kept in one write, so a task deleted
        // since it was checked leaves nothing behind.
        let run_id = RunId::generate();
        tracing::Span::current().record("run_id", tracing::field::display(run_id));
        let run = match self
            .repo
            .insert_run(NewRun {
                id: run_id,
                flow: built_in.name,
                request: &request,
                user_id: &actor,
                task_id,
                repository: repository.as_ref(),
                created_in_person,
                control_file_patterns: &control_file_patterns,
                control_files_team_id,
                standing_rules,
            })
            .await
            .map_err(internal)?
        {
            RunInsert::Inserted(run) => *run,
            RunInsert::NoSuchTask => return Err(FactoryError::UnknownTask),
        };

        // The Run is recorded before its workflow starts, so the workflow can
        // record its Steps against it.
        let start = RunStart { run_id, flow };
        if let Err(error) = self.workflows.start(&start).await {
            self.forget_unstarted(run_id).await;
            return Err(error.into());
        }

        self.detail(run, Vec::new(), Vec::new()).await
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
        let review_rounds = self.repo.review_rounds(id).await.map_err(internal)?;
        self.detail(run, steps, review_rounds).await
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
        let mut rounds = self.repo.runs_review_rounds(&ids).await.map_err(internal)?;
        // Runs on one base share what was read of it.
        let mut known = BTreeMap::new();
        let mut details = Vec::with_capacity(runs.len());
        for run in runs {
            let steps = steps.remove(&run.id).unwrap_or_default();
            let review_rounds = rounds.remove(&run.id).unwrap_or_default();
            let base = self.base_shown(&run, &steps, &mut known).await?;
            details.push(RunDetail {
                run,
                steps,
                main_hold: base.hold,
                base_required_checks: base.required,
                review_rounds,
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
        approved_control_files: Option<Vec<String>>,
    ) -> Result<(), FactoryError> {
        let note_bytes = note.as_ref().map_or(0, String::len);
        if note_bytes > MAX_NOTE_BYTES {
            return Err(FactoryError::NoteTooLarge {
                max: MAX_NOTE_BYTES,
            });
        }
        self.known(&actor).await?;
        self.not_an_agent(&actor).await?;
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
        // An agent's question is answered in the owner's own words: the
        // agent goes on with them.
        let blank = note.as_deref().is_none_or(|note| note.trim().is_empty());
        if gate.is_some_and(|gate| gate.kind.answered_only_by_a_person()) && blank {
            return Err(FactoryError::AnswerMissing);
        }

        // A Tampered Run merges only once its owner approves the Control
        // Files its branch changed unasked: approving its Gate must list the
        // ones they saw, which must be the Run's, and the answer keeps them
        // for the merge to check. The pull request Steps keep them before
        // the Gate opens, and nothing changes them while the Run waits.
        let tampered = run
            .control_files
            .as_ref()
            .filter(|changes| changes.is_tampered());
        let approved_control_files = match tampered {
            Some(changes) if answer.as_str() == APPROVE => {
                let mut seen =
                    approved_control_files.ok_or(FactoryError::ControlFilesNotApproved)?;
                let mut kept = changes.tampered.clone();
                seen.sort();
                kept.sort();
                if seen != kept {
                    return Err(FactoryError::ControlFilesNotApproved);
                }
                Some(changes.tampered.clone())
            }
            _ => None,
        };

        let answer = AnswerGate {
            seq,
            answer,
            note,
            user_id: actor,
            approved_control_files,
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
        // A question is put to a person on a Gate, which keeps what fits
        // on one: the agent hears now that it must be shorter.
        let question = hand_back.output.trim();
        if hand_back.outcome == Outcome::question()
            && (question.is_empty() || question.len() > MAX_QUESTION_BYTES)
        {
            return Err(FactoryError::QuestionSize {
                max: MAX_QUESTION_BYTES,
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
    async fn control_files(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<ControlFilesSettings, FactoryError> {
        let (team, own) = self.team_control_files(actor).await?;
        Ok(ControlFilesSettings {
            is_default: own.is_none(),
            patterns: own.unwrap_or_else(ControlFiles::default_patterns),
            team_id: team.map(|team| team.team_id),
            can_edit: team.is_some_and(|team| team.admin),
        })
    }

    #[tracing::instrument(skip(self, actor, patterns))]
    async fn set_control_files(
        &self,
        actor: MacroUserIdStr<'static>,
        patterns: Option<Vec<String>>,
    ) -> Result<ControlFilesSettings, FactoryError> {
        if let Some(patterns) = &patterns {
            ControlFiles::parse(patterns)?;
        }
        let team = self.administered_team(&actor).await?;
        self.repo
            .set_team_control_files(team.team_id, patterns.as_deref())
            .await
            .map_err(internal)?;
        self.control_files(&actor).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn standing_rules(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<StandingRulesSettings, FactoryError> {
        let team = self.repo.team_of(actor).await.map_err(internal)?;
        // An agent's account never changes them, whatever its team role.
        let agent = self.repo.is_agent_account(actor).await.map_err(internal)?;
        let (own, projects) = match team {
            Some(team) => (
                self.repo.team_rules(team.team_id).await.map_err(internal)?,
                self.repo
                    .list_project_rules(team.team_id)
                    .await
                    .map_err(internal)?,
            ),
            None => (None, Vec::new()),
        };
        Ok(StandingRulesSettings {
            is_default: own.is_none(),
            definition: own.unwrap_or(OrchestratorDefinition::DEFAULT),
            projects,
            team_id: team.map(|team| team.team_id),
            can_edit: !agent && team.is_some_and(|team| team.admin),
        })
    }

    #[tracing::instrument(skip(self, actor))]
    async fn set_standing_rules(
        &self,
        actor: MacroUserIdStr<'static>,
        definition: Option<OrchestratorDefinition>,
    ) -> Result<StandingRulesSettings, FactoryError> {
        let team = self.administered_team(&actor).await?;
        self.repo
            .set_team_rules(team.team_id, definition)
            .await
            .map_err(internal)?;
        self.standing_rules(&actor).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn set_project_rules(
        &self,
        actor: MacroUserIdStr<'static>,
        project_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> Result<StandingRulesSettings, FactoryError> {
        let team = self.administered_team(&actor).await?;
        // A Project the admin may not edit is not theirs to give rules: it
        // reads as one that does not exist.
        if !self
            .tasks
            .may_edit_project(&actor, project_id)
            .await
            .map_err(internal)?
        {
            return Err(FactoryError::UnknownProject);
        }
        match self
            .repo
            .set_project_rules(team.team_id, project_id, definition)
            .await
            .map_err(internal)?
        {
            ProjectRulesSet::Set => self.standing_rules(&actor).await,
            ProjectRulesSet::NoSuchProject => Err(FactoryError::UnknownProject),
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

/// How many questions an agent Step may ask the Run's owner, unless the
/// service is told otherwise: the next one ends the Run for a person.
pub const DEFAULT_QUESTIONS: u8 = 2;

/// What a Run's worker does for its workflow: run Steps, and record Steps,
/// Gates and the Run's end, each on its own.
#[derive(Debug, Clone)]
pub struct StepServiceImpl<R, E> {
    repo: R,
    executor: E,
}

impl<R, E> StepServiceImpl<R, E>
where
    R: RunRepository + ReviewRoundRepository + MainHoldRepository,
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
    R: RunRepository + ReviewRoundRepository + MainHoldRepository,
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

        // The Step reads what the Run's Steps before it produced: a
        // reviewer, the round it reviews in, and any other Step the latest
        // round that ended before it, whose Blockers a fix reads.
        let earlier: Vec<_> = self
            .repo
            .list_steps(request.key.run_id)
            .await
            .map_err(internal)?
            .into_iter()
            .filter(|step| step.seq < request.key.seq && step.end.is_some())
            .collect();
        let rounds = self
            .repo
            .review_rounds(request.key.run_id)
            .await
            .map_err(internal)?;
        let review = review_of(&rounds, &request)?;
        let input = StepInput {
            key: request.key,
            role: request.role.as_ref(),
            owner: &run.user_id,
            request: &run.request,
            created_in_person: run.created_in_person,
            control_file_patterns: run.control_file_patterns.as_deref(),
            task_id: run.task_id,
            repository: run.repository.as_ref(),
            earlier: &earlier,
            review,
            reviewer: request.round.is_some(),
            may_ask: request.may_ask,
            questions_left: request.questions_left,
            resumes: request.resumes,
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
                answer.approved_control_files.as_deref(),
            )
            .await
            .map_err(internal)?;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %ruling.key.run_id, seq = ruling.key.seq, kind = %ruling.kind))]
    async fn rule_gate(&self, ruling: GateRuling) -> Result<Option<RuleAnswer>, StepError> {
        let key = ruling.key;
        // An earlier attempt may have answered the Gate before its result
        // was lost: that answer stands, whatever the Run reads now.
        let steps = self.repo.list_steps(key.run_id).await.map_err(internal)?;
        let gate = steps
            .iter()
            .find(|step| step.seq == key.seq)
            .and_then(|step| step.gate.as_ref());
        // An agent's question is the Run's owner's alone to answer, whatever
        // the ruling names.
        if gate.is_some_and(|gate| gate.kind.answered_only_by_a_person()) {
            return Ok(None);
        }
        let answered = gate.and_then(|gate| gate.answer.as_ref());
        if let Some(answer) = answered {
            return Ok(answer.rule.map(|rule| RuleAnswer {
                rule,
                answer: answer.answer.clone(),
            }));
        }

        let run = self
            .repo
            .get_run(key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        if run.end.is_some() {
            return Ok(None);
        }
        let Some(rules) = run.standing_rules else {
            return Ok(None);
        };
        let held = match (&run.repository, ruling.kind) {
            (Some(base), RuleGateKind::Merge) => {
                self.repo.main_hold(base).await.map_err(internal)?.is_some()
            }
            _ => false,
        };
        let facts = GateFacts {
            kind: ruling.kind,
            fix_rounds: ruling.fix_rounds,
            fix_limit: ruling.fix_limit,
            tampered: run
                .control_files
                .as_ref()
                .is_some_and(|changes| changes.is_tampered()),
            held,
        };
        let Some(ruled) = rule_answer(Some(rules.definition), &facts, &ruling.answers) else {
            return Ok(None);
        };
        // The Gate keeps its first answer: what it keeps is what the Run
        // routes, read back after the write.
        let kept = self
            .repo
            .answer_gate_by_rule(key, &ruled.answer, ruled.rule)
            .await
            .map_err(internal)?
            .ok_or_else(|| StepError::Internal(rootcause::report!("the gate is not recorded")))?;
        let Some(rule) = kept.rule else {
            return Err(StepError::Final(rootcause::report!(
                "a person answered the gate before the run took answers"
            )));
        };
        tracing::info!(%rule, answer = %kept.answer, "a standing rule answered a gate");
        Ok(Some(RuleAnswer {
            rule,
            answer: kept.answer,
        }))
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

/// The review round `request`'s Step reads: a reviewer's own round, which
/// must be recorded, and for any other Step the latest round that ended
/// before it, if any.
fn review_of<'a>(
    rounds: &'a [ReviewRound],
    request: &StepRequest,
) -> Result<Option<&'a ReviewRound>, StepError> {
    match request.round {
        Some(seq) => rounds
            .iter()
            .find(|round| round.seq == seq)
            .map(Some)
            .ok_or_else(|| {
                StepError::Final(rootcause::report!("no review round starts at step {seq}"))
            }),
        None => Ok(rounds
            .iter()
            .rev()
            .find(|round| round.seq < request.key.seq && round.end.is_some())),
    }
}
