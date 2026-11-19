//! The factory's use cases: starting and reading Runs, and running their
//! Steps.

mod board;
mod definition;
#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU8;
use std::sync::Arc;

use agent_session::domain::session_task::TaskDocumentId;
use chrono::Utc;
use factory_records::domain::automations::{
    Automation, AutomationRepository, AutomationStart, RunAutomation,
};
use factory_records::domain::board::RunBoardRepository;
use factory_records::domain::evaluation::{BENCHMARK_FLOW, EvaluationRepository, TrialKey};
use factory_records::domain::flaws::FlawRepository;
use factory_records::domain::main_hold::{BaseState, MainHoldRepository};
use factory_records::domain::models::{
    EntryId, FlawMarker, HandBack, KeptFlow, MAX_NOTE_BYTES, MAX_OUTPUT_BYTES, MAX_QUESTION_BYTES,
    MAX_REQUEST_BYTES, NewRun, NewSteer, Outcome, QueueHold, Repository, Run, RunId, RunInsert,
    RunOutcome, SteerRefusal, Step, StepKey, TaskLink, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::{ReviewRound, ReviewRoundRepository};
use factory_records::domain::standing_rules::{
    MAX_QUEUE_LIMIT, MIN_QUEUE_LIMIT, OrchestratorDefinition, ProjectRulesSet, RuleGateKind,
    RulesSource, RunRules, StandingRulesRepository,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::board::{BoardFilter, BoardPage, PullRequestRuns};
use crate::domain::built_in_flows::BUILT_IN_FLOWS;
use crate::domain::control_files::ControlFiles;
use crate::domain::definition::DEFINITION_FILE_PATTERNS;
use crate::domain::evaluation::{EvaluationDefinition, TeamEvaluation};
use crate::domain::flaws::mark_flawed;
use crate::domain::flow::Flow;
use crate::domain::gate::{
    AnswerGate, GateAnswered, GateOpening, GateRuling, GateTelling, ReplyGate,
};
use crate::domain::gate_owner::{
    OwnerNote, gate_notice, gate_told, is_factory_comment, notes_since, notice_id, stand_in_id,
    told_id,
};
use crate::domain::models::{
    Answerer, CodeRepository, ControlFilesSettings, DefinitionSettings, FactoryError, FlowSummary,
    PersonOnly, RunDetail, RunFinish, RunStart, SessionSteer, SessionStepRef,
    StandingRulesSettings, StepEnding, StepError, StepInput, StepReport, StepRequest, StepStart,
};
use crate::domain::ports::{
    AgentFailure, AnswerDelivery, FactoryService, GateNotifier, NoOwnerGates, NoTaskThread,
    PullRequests, Retryable, RunWorkflows, RunWorkflowsError, StepExecutor, StepService,
    TaskAccess, TaskDirectory, TaskNotes, TaskThread, UserDirectory,
};
use crate::domain::standing_rules::{
    GateFacts, RuleAnswer, kind_answers, offers_kind_answers, repeated_blockers, rule_answer,
    rule_fix_limit,
};
use crate::domain::step_terminal::{
    PAUSE_RAN_OUT_OUTPUT, STOPPED_OUTPUT, TerminalRequest, note_prompt, paused_step_ends_by,
    resume_prompt,
};
use crate::domain::task_thread::{RunUpdate, post_update};
use definition::PinnedDefinition;

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> rootcause::Report {
    rootcause::Report::new(error).into_dynamic()
}

/// `steps`, each running paused Step saying when it ends unless resumed.
fn with_pause_ends(mut steps: Vec<Step>) -> Vec<Step> {
    for step in &mut steps {
        if step.end.is_none() && step.terminal.paused_at.is_some() {
            step.terminal.ends_by = Some(paused_step_ends_by(step.started_at));
        }
    }
    steps
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

/// The most Runs waiting on their owner that are listed for them.
pub const WAITING_RUNS_LISTED: u16 = 50;

/// How old a Run with no Step and no workflow must be before it counts as
/// one whose workflow never started.
const UNSTARTED_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

/// A Run to record and start: what [`FactoryService::create_run`] takes,
/// and the Automation that starts it, if one does.
struct RunOrder<'a> {
    actor: MacroUserIdStr<'static>,
    flow: String,
    request: String,
    task_id: Option<Uuid>,
    repo_url: Option<String>,
    in_person: bool,
    automation: Option<AutomationStart<'a>>,
    /// The Benchmark trial it is, if a suite starts it, and the commit its
    /// write starts from.
    trial: Option<(TrialKey, String)>,
}

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
    task_views: board::TaskViews,
    evaluation: Arc<EvaluationDefinition>,
}

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository
        + MainHoldRepository
        + ReviewRoundRepository
        + StandingRulesRepository
        + AutomationRepository
        + FlawRepository
        + RunBoardRepository
        + EvaluationRepository,
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
            task_views: board::TaskViews::default(),
            evaluation: Arc::default(),
        }
    }

    /// Keep the board's task views for `kept` each, in place of 30 s.
    #[cfg(test)]
    pub(crate) fn with_task_views_kept(mut self, kept: std::time::Duration) -> Self {
        self.task_views = board::TaskViews::kept_for(kept);
        self
    }

    /// Assign the Runs it starts to the arms of `evaluation`'s
    /// Experiments, in place of none.
    pub fn with_evaluation(mut self, evaluation: Arc<EvaluationDefinition>) -> Self {
        self.evaluation = evaluation;
        self
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

    /// The Project the task `task_id` is in, as `user_id` sees it: none
    /// without a task, or when it cannot be read now, which leaves a Run
    /// its team's rules rather than refusing it.
    async fn run_project(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Option<Uuid>,
    ) -> Option<Uuid> {
        let task_id = task_id?;
        match self.tasks.task_project(user_id, task_id).await {
            Ok(project) => project,
            Err(error) => {
                tracing::warn!(
                    ?error,
                    %task_id,
                    "the run task's project could not be read: the run takes its team's standing rules"
                );
                None
            }
        }
    }

    /// The Standing Rules a Run is created with, in the Project `project`
    /// when its task is in one: its team's Factory Definition's when the
    /// team names one; else the Project's own, when its owner's team set
    /// rules for it, else the team's, else the factory's default.
    async fn run_rules(
        &self,
        team: Option<TeamMembership>,
        project: Option<Uuid>,
        definition: Option<&PinnedDefinition>,
    ) -> Result<RunRules, FactoryError> {
        // A team's Factory Definition holds its rules: neither the team's
        // settings nor a Project's own apply, as they change without a
        // pull request.
        if let Some(pinned) = definition {
            return Ok(RunRules {
                definition: pinned
                    .definition
                    .rules
                    .standing_rules
                    .unwrap_or(OrchestratorDefinition::DEFAULT),
                source: RulesSource::Definition,
                project_id: None,
            });
        }
        let Some(team) = team else {
            return Ok(RunRules::DEFAULT);
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

    /// The Run `id`, when `actor` may read it: their own, or a team
    /// member's on a task they can see. Only its owner acts on it.
    async fn readable_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> Result<Run, FactoryError> {
        let run = self
            .repo
            .get_run(id)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::NotFound)?;
        if run.user_id == *actor || self.sees_teammates_run(actor, &run).await? {
            return Ok(run);
        }
        Err(FactoryError::NotFound)
    }

    /// Whether `steps`' latest Step is at a merge: its entry runs
    /// `ready_to_merge` or `merge` in `flow`, the Run's own.
    fn at_merge(steps: &[Step], flow: &Flow) -> bool {
        let Some(latest) = steps.last() else {
            return false;
        };
        flow.entries.iter().any(|entry| {
            entry.id == latest.entry && matches!(entry.step.as_str(), "ready_to_merge" | "merge")
        })
    }

    /// Whether `run` may be at its merge, so what it shows of its base
    /// branch needs its Flow: it works on a repository, goes, and took a
    /// Step.
    fn may_be_at_merge(run: &Run, steps: &[Step]) -> bool {
        run.repository.is_some() && run.end.is_none() && !steps.is_empty()
    }

    /// What the Run shows of the base branch it merges to: nothing unless
    /// it goes and is at its merge in `flow`, its own, so only a Run about
    /// to merge shows a hold. `known` keeps what was read of each base,
    /// for a list of Runs.
    async fn base_shown(
        &self,
        run: &Run,
        steps: &[Step],
        flow: Option<&Flow>,
        known: &mut BTreeMap<(String, String), BaseState>,
    ) -> Result<BaseState, FactoryError> {
        let Some(repository) = run.repository.as_ref() else {
            return Ok(BaseState::default());
        };
        let at_merge = flow.is_some_and(|flow| Self::at_merge(steps, flow));
        if !Self::may_be_at_merge(run, steps) || !at_merge {
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
        let flow = if Self::may_be_at_merge(&run, &steps) {
            self.run_flow(run.id).await?
        } else {
            None
        };
        let base = self
            .base_shown(&run, &steps, flow.as_ref(), &mut BTreeMap::new())
            .await?;
        Ok(RunDetail {
            run,
            steps: with_pause_ends(steps),
            main_hold: base.hold,
            base_required_checks: base.required,
            review_rounds,
        })
    }

    /// `runs`, each with its Steps, its review rounds, and what was read of
    /// its base branch, in their order.
    async fn details(&self, runs: Vec<Run>) -> Result<Vec<RunDetail>, FactoryError> {
        let ids: Vec<RunId> = runs.iter().map(|run| run.id).collect();
        let mut steps = self.repo.list_runs_steps(&ids).await.map_err(internal)?;
        let mut rounds = self.repo.runs_review_rounds(&ids).await.map_err(internal)?;
        // The Flows of the Runs that may be at their merge, in one read.
        let at_merge_ids: Vec<RunId> = runs
            .iter()
            .filter(|run| {
                let taken = steps.get(&run.id).map_or(&[][..], Vec::as_slice);
                Self::may_be_at_merge(run, taken)
            })
            .map(|run| run.id)
            .collect();
        let flows = self.runs_flows(&at_merge_ids).await?;
        // Runs on one base share what was read of it.
        let mut known = BTreeMap::new();
        let mut details = Vec::with_capacity(runs.len());
        for run in runs {
            let steps = with_pause_ends(steps.remove(&run.id).unwrap_or_default());
            let review_rounds = rounds.remove(&run.id).unwrap_or_default();
            let base = self
                .base_shown(&run, &steps, flows.get(&run.id), &mut known)
                .await?;
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

    /// The built-in Flows whose Runs hold a place in their owner's Queue
    /// before their first Gate: those with a Gate of their own. A Run of
    /// another Flow, such as `reply`, holds one only once it asks something.
    fn asking_flows(&self) -> Vec<&'static str> {
        BUILT_IN_FLOWS
            .into_iter()
            .filter(|flow| {
                flow.read(|step| self.executor.serves(step))
                    .is_ok_and(|read| read.entries.iter().any(|entry| entry.gate.is_some()))
            })
            .map(|flow| flow.name)
            .collect()
    }

    /// The kind of Gate the entry `entry` asks in `flow`, the Run's own,
    /// when an agent answering for the Run's owner may answer it, with the
    /// most visits the entry runs: a Gate of a kind, not one kept for a
    /// person.
    fn agent_answerable_entry(
        flow: &Flow,
        entry: &EntryId,
    ) -> Option<(RuleGateKind, Option<NonZeroU8>)> {
        let asking = flow.entries.iter().find(|asking| asking.id == *entry)?;
        let gate = asking.gate.as_ref().filter(|gate| !gate.person_only)?;
        Some((gate.kind?, asking.max_visits))
    }

    /// Fails unless an agent answering for the Run's owner from the session
    /// `session_id` may give `answer` to the Gate of `run`'s Step `seq`,
    /// which must still wait for an answer. It answers only a Flow's Gate
    /// of a kind a Standing Rule could decide, and never from a session a
    /// factory Step works in. It may always send work back (`revise`,
    /// `stop`). It gives the approving answer (`approve`, `fix`) only where
    /// the Run's Orchestrator Definition lets it, for that kind, never for
    /// a merge, never approving a Tampered Run, and never sending Blockers
    /// to a fix the `fix_blockers` rule would leave to the owner.
    async fn agent_may_answer(
        &self,
        run: &Run,
        steps: &[Step],
        seq: u16,
        answer: &Outcome,
        approved_control_files: bool,
        session_id: Uuid,
    ) -> Result<(), FactoryError> {
        // The agents doing a Run's Steps work as its owner too: one could
        // otherwise approve the plan it wrote.
        if self
            .repo
            .session_step(session_id)
            .await
            .map_err(internal)?
            .is_some()
        {
            return Err(PersonOnly::StepSession.into());
        }

        // The agent names the Gate it decided on: one answered since, by
        // anyone, takes no answer from it.
        let waiting = steps
            .iter()
            .find(|step| step.seq == seq)
            .and_then(|step| Some((step, step.gate.as_ref()?)))
            .filter(|(_, gate)| gate.answer.is_none());
        let Some((step, gate)) = waiting else {
            return Err(FactoryError::AnswerRefused(
                "the run no longer waits on that gate".to_string(),
            ));
        };
        if gate.kind.answered_only_by_a_person() {
            return Err(PersonOnly::Question.into());
        }
        // Nor the Gate of an agent that stopped without a result: whether
        // to try again is the owner's call.
        if step
            .end
            .as_ref()
            .is_some_and(|end| end.outcome == Outcome::no_result())
        {
            return Err(PersonOnly::PersonsGate.into());
        }
        // The kind comes from the Run's own Flow, as the workflow and the
        // Standing Rules read it: never from another Flow with an entry of
        // the same id.
        let flow = self.run_flow(run.id).await?;
        let (kind, max_visits) = flow
            .as_ref()
            .and_then(|flow| Self::agent_answerable_entry(flow, &step.entry))
            .filter(|(kind, _)| offers_kind_answers(*kind, &gate.answers))
            .ok_or(PersonOnly::PersonsGate)?;
        if approved_control_files {
            return Err(PersonOnly::ControlFiles.into());
        }

        // Sending work back changes nothing a person has not seen.
        let [approving, _] = kind_answers(kind);
        if answer.as_str() != approving {
            return Ok(());
        }

        let tampered = run
            .control_files
            .as_ref()
            .is_some_and(|changes| changes.is_tampered());
        match kind {
            RuleGateKind::Merge => return Err(PersonOnly::Merge.into()),
            RuleGateKind::Plan if tampered => return Err(PersonOnly::ControlFiles.into()),
            RuleGateKind::Plan | RuleGateKind::Blockers => {}
        }
        let Some(rules) = run.standing_rules else {
            return Err(PersonOnly::NotAllowed.into());
        };
        if !rules.definition.orchestrator_approves(kind) {
            return Err(PersonOnly::NotAllowed.into());
        }

        // A Blockers Gate the `fix_blockers` rule would leave to the owner
        // is theirs: past the fix rounds in a row it may send, and, with
        // the owner section on, Blockers found again unchanged after a fix.
        if kind == RuleGateKind::Blockers {
            let fix_rounds = step.visit.saturating_sub(1);
            if fix_rounds >= rule_fix_limit(max_visits) {
                return Err(PersonOnly::OwnersBlockers.into());
            }
            if rules.definition.owner_gates {
                let rounds = self.repo.review_rounds(run.id).await.map_err(internal)?;
                if repeated_blockers(&rounds, steps, seq) {
                    return Err(PersonOnly::OwnersBlockers.into());
                }
            }
        }
        Ok(())
    }

    /// Record the Run `order` asks for and start it, as
    /// [`FactoryService::create_run`] does, keeping the Automation that
    /// starts it if one does.
    #[tracing::instrument(skip_all, fields(run_id = tracing::field::Empty))]
    async fn start_run(&self, order: RunOrder<'_>) -> Result<RunDetail, FactoryError> {
        let RunOrder {
            actor,
            flow,
            request,
            task_id,
            repo_url,
            in_person,
            automation,
            trial,
        } = order;
        // A Benchmark's Flow runs only as a suite's trial, and a trial only
        // on it.
        if (flow == BENCHMARK_FLOW) != trial.is_some() {
            return Err(FactoryError::UnknownFlow);
        }
        // A team that names a Factory Definition starts its Runs of the
        // Definition's Flows, read at its default branch's head now: the Run
        // keeps that commit, and the Definition's Agents.
        let team = self.repo.team_of(&actor).await.map_err(internal)?;
        let definition = self.team_definition(&actor, team).await?;
        let flow_name = flow;
        // A Benchmark trial walks the built-in benchmark Flow whatever Flows
        // the team's Definition holds, none of which takes its name; it
        // still keeps the Definition, whose Agents its Steps take.
        let flows_of = definition.as_ref().filter(|_| trial.is_none());
        let flow = self.flow_named(&flow_name, flows_of)?;
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
        // An Automation starts no Run on a task while one runs there, whoever
        // started it; checked only once its creator may see the task, so a
        // firing on a task they cannot see says nothing of it. Two firings
        // that race past this are held to one by the Runs' index
        // (`RunInsert::TaskBusy`).
        if automation.is_some()
            && let Some(task_id) = task_id
            && self
                .repo
                .running_run_on_task(task_id)
                .await
                .map_err(internal)?
                .is_some()
        {
            return Err(FactoryError::TaskBusy);
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
        let own = match (&definition, team) {
            (Some(pinned), _) => pinned.definition.rules.control_files.clone(),
            (None, Some(team)) => self
                .repo
                .team_control_files(team.team_id)
                .await
                .map_err(internal)?,
            (None, None) => None,
        };
        let control_files_team_id = team.filter(|_| own.is_some()).map(|team| team.team_id);
        let mut control_file_patterns = own.unwrap_or_else(ControlFiles::default_patterns);
        // A Run on its team's Definition repository changes the factory's
        // own rules: those files are Control Files too, whatever the list.
        let on_definition = definition.as_ref().zip(repository.as_ref());
        // GitHub names a repository whatever the case of its owner and name.
        let same = |(pinned, repository): (&PinnedDefinition, &Repository)| {
            pinned.repo_url.eq_ignore_ascii_case(&repository.url)
        };
        if on_definition.is_some_and(same) {
            for pattern in DEFINITION_FILE_PATTERNS {
                if !control_file_patterns.iter().any(|kept| kept == pattern) {
                    control_file_patterns.push(pattern.to_string());
                }
            }
        }
        let created_in_person =
            in_person && !self.repo.is_agent_account(&actor).await.map_err(internal)?;
        // The Run keeps its Standing Rules the same way: a change to the
        // team's or the Project's rules, or a task linked later, does not
        // change which of its Gates a rule answers.
        let project_id = self.run_project(&actor, task_id).await;
        let standing_rules = self
            .run_rules(team, project_id, definition.as_ref())
            .await?;
        // The Run and its task are kept in one write, so a task deleted
        // since it was checked leaves nothing behind. Once the owner holds
        // as many places as the Queue limit of the rules it would start
        // under, counted in the same write, no new Run starts, whoever
        // asks: the person, an agent for them, or an Automation; starts at
        // the same time are counted one after the other.
        // The Run keeps the Flow it walks, and whether it asks before its
        // first Gate: what its Gates are, and whether it holds a place in
        // the Queue, are read from it, whatever its team's Definition says
        // later. Runs kept before are counted by their Flow's name.
        let asking_flows = self.asking_flows();
        let kept = serde_json::to_value(&flow).map_err(internal)?;
        let asks = flow.entries.iter().any(|entry| entry.gate.is_some());
        let hold = QueueHold {
            limit: standing_rules.definition.queue_limit,
            asking_flows: &asking_flows,
        };
        let rules_source = standing_rules.source;
        let run_id = RunId::generate();
        tracing::Span::current().record("run_id", tracing::field::display(run_id));
        let run = match self
            .repo
            .insert_run_in_queue(
                NewRun {
                    id: run_id,
                    flow: &flow_name,
                    request: &request,
                    user_id: &actor,
                    task_id,
                    project_id,
                    repository: repository.as_ref(),
                    created_in_person,
                    control_file_patterns: &control_file_patterns,
                    control_files_team_id,
                    standing_rules,
                    automation,
                    definition: definition.as_ref().map(PinnedDefinition::kept),
                    kept_flow: Some(KeptFlow {
                        definition: &kept,
                        asks,
                    }),
                },
                hold,
            )
            .await
            .map_err(internal)?
        {
            RunInsert::Inserted(run) => *run,
            RunInsert::NoSuchTask => return Err(FactoryError::UnknownTask),
            RunInsert::TaskBusy => return Err(FactoryError::TaskBusy),
            RunInsert::FiringStarted => return Err(FactoryError::FiringRepeated),
            RunInsert::QueueFull(places) => {
                return Err(FactoryError::QueueFull {
                    limit: hold.limit.unwrap_or_default(),
                    set_by: rules_source,
                    waiting: places.waiting,
                    starting: places.starting,
                });
            }
        };

        // The Run keeps its trial, or the arms of its Flow's Experiments,
        // before its workflow starts, so its first Step already runs on
        // their Agents. A Run that cannot keep them does not start. A
        // team's Definition holds its Experiments; a team with none takes
        // the deployment's.
        let kept = match &trial {
            Some((trial, base_commit)) => self.repo.insert_trial(trial, run_id, base_commit).await,
            None => {
                let evaluation = match &definition {
                    Some(pinned) => &pinned.definition.evaluation,
                    None => &*self.evaluation,
                };
                let arms = evaluation.assign_arms(&flow_name, task_id);
                self.repo.record_arms(run_id, &arms).await
            }
        };
        if let Err(error) = kept {
            self.forget_unstarted(run_id).await;
            return Err(FactoryError::Internal(internal(error)));
        }

        // The Run is recorded before its workflow starts, so the workflow can
        // record its Steps against it.
        let start = RunStart { run_id, flow };
        if let Err(error) = self.workflows.start(&start).await {
            self.forget_unstarted(run_id).await;
            return Err(error.into());
        }

        self.detail(run, Vec::new(), Vec::new()).await
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
    R: RunRepository
        + MainHoldRepository
        + ReviewRoundRepository
        + StandingRulesRepository
        + AutomationRepository
        + FlawRepository
        + RunBoardRepository
        + EvaluationRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    async fn list_flows(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<Vec<FlowSummary>, FactoryError> {
        let team = self.repo.team_of(actor).await.map_err(internal)?;
        if let Some(pinned) = self.team_definition(actor, team).await? {
            let flows = pinned.definition.flows.into_iter();
            return Ok(flows
                .map(|(name, flow)| FlowSummary {
                    name,
                    description: flow.description,
                    repository: flow.repository,
                })
                .collect());
        }

        // A Flow that names a Step the worker does not serve yet is not
        // offered: a Run of it would be refused.
        Ok(BUILT_IN_FLOWS
            .into_iter()
            .filter(|flow| flow.name != BENCHMARK_FLOW)
            .filter_map(|flow| {
                let read = flow.read(|step| self.executor.serves(step)).ok()?;
                Some(FlowSummary {
                    name: flow.name.to_string(),
                    description: read.description,
                    repository: read.repository,
                })
            })
            .collect())
    }

    async fn create_run(
        &self,
        actor: MacroUserIdStr<'static>,
        flow: String,
        request: String,
        task_id: Option<Uuid>,
        repo_url: Option<String>,
        in_person: bool,
    ) -> Result<RunDetail, FactoryError> {
        self.start_run(RunOrder {
            actor,
            flow,
            request,
            task_id,
            repo_url,
            in_person,
            automation: None,
            trial: None,
        })
        .await
    }

    async fn benchmark_base(
        &self,
        actor: &MacroUserIdStr<'static>,
        repo_url: &str,
    ) -> Result<String, FactoryError> {
        self.known(actor).await?;
        let repository = self.reachable_repository(actor, repo_url).await?;
        let code = CodeRepository::parse(repo_url).ok_or(FactoryError::InvalidRepository)?;
        self.github
            .branch_head(actor, &code, &repository.base_branch)
            .await
            .map_err(internal)?
            .ok_or(FactoryError::UnreachableRepository)
    }

    async fn team_evaluation(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<Option<TeamEvaluation>, FactoryError> {
        let team = self.repo.team_of(actor).await.map_err(internal)?;
        let pinned = self.team_definition(actor, team).await?;
        Ok(pinned.map(|pinned| TeamEvaluation {
            evaluation: pinned.definition.evaluation,
            agents: pinned.definition.agents,
        }))
    }

    async fn create_trial_run(
        &self,
        actor: MacroUserIdStr<'static>,
        flow: String,
        request: String,
        repo_url: String,
        trial: TrialKey,
        base_commit: String,
    ) -> Result<RunDetail, FactoryError> {
        self.start_run(RunOrder {
            actor,
            flow,
            request,
            task_id: None,
            repo_url: Some(repo_url),
            in_person: false,
            automation: None,
            trial: Some((trial, base_commit)),
        })
        .await
    }

    async fn check_run_order(
        &self,
        actor: &MacroUserIdStr<'static>,
        flow: &str,
        repo_url: Option<&str>,
    ) -> Result<(), FactoryError> {
        if flow == BENCHMARK_FLOW {
            return Err(FactoryError::UnknownFlow);
        }
        let team = self.repo.team_of(actor).await.map_err(internal)?;
        let definition = self.team_definition(actor, team).await?;
        let flow = self.flow_named(flow, definition.as_ref())?;
        match (flow.repository, repo_url) {
            (true, None) => return Err(FactoryError::RepositoryRequired),
            (false, Some(_)) => return Err(FactoryError::RepositoryNotUsed),
            _ => {}
        }
        self.known(actor).await?;
        if let Some(repo_url) = repo_url {
            self.reachable_repository(actor, repo_url).await?;
        }
        Ok(())
    }

    async fn create_automation_run(
        &self,
        automation: &Automation,
        firing_key: &str,
        request: String,
        task_id: Option<Uuid>,
    ) -> Result<RunDetail, FactoryError> {
        let started_by = RunAutomation {
            id: automation.id,
            name: automation.name.clone(),
            trigger: automation.trigger.kind(),
        };
        // An Automation acts as its creator, never in their signed-in
        // session: its request names no Control File.
        self.start_run(RunOrder {
            actor: automation.created_by.clone(),
            flow: automation.flow.clone(),
            request,
            task_id,
            repo_url: automation.repo_url.clone(),
            in_person: false,
            automation: Some(AutomationStart {
                automation: &started_by,
                firing_key,
            }),
            trial: None,
        })
        .await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn get_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
    ) -> Result<RunDetail, FactoryError> {
        let mut run = self.readable_run(actor, id).await?;
        let steps = self.repo.list_steps(id).await.map_err(internal)?;

        // A Run kept when its workflow's start could not be checked, and
        // whose workflow never came, would wait forever: once it is old
        // enough that a started workflow would have taken a Step, and
        // Temporal says it has no workflow, its owner's read ends it
        // `infrastructure`, as nothing of its work ever ran. A team
        // member's read changes nothing.
        let stale = Utc::now() - run.created_at > UNSTARTED_AFTER;
        if run.user_id == *actor
            && run.end.is_none()
            && steps.is_empty()
            && stale
            && let Ok(false) = self.workflows.exists(id).await
        {
            self.repo
                .end_run(id, RunOutcome::Infrastructure)
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
        self.details(runs).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn list_waiting_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<Vec<RunDetail>, FactoryError> {
        let runs = self
            .repo
            .list_waiting_runs(actor, WAITING_RUNS_LISTED)
            .await
            .map_err(internal)?;
        self.details(runs).await
    }

    #[tracing::instrument(skip(self, actor, filter), fields(scope = ?filter.scope, state = ?filter.state))]
    async fn board(
        &self,
        actor: &MacroUserIdStr<'static>,
        filter: BoardFilter,
    ) -> Result<BoardPage, FactoryError> {
        self.board_page(actor, filter).await
    }

    #[tracing::instrument(skip(self, actor, urls), fields(urls = urls.len()))]
    async fn pull_request_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
        urls: Vec<String>,
    ) -> Result<PullRequestRuns, FactoryError> {
        self.pull_request_runs_of(actor, urls).await
    }

    async fn list_starting_runs(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<Vec<RunDetail>, FactoryError> {
        let asking_flows = self.asking_flows();
        let runs = self
            .repo
            .list_starting_runs(actor, &asking_flows, WAITING_RUNS_LISTED)
            .await
            .map_err(internal)?;
        self.details(runs).await
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

    #[tracing::instrument(skip(self, actor))]
    async fn session_step(
        &self,
        actor: &MacroUserIdStr<'static>,
        session_id: Uuid,
    ) -> Result<SessionStepRef, FactoryError> {
        let step = self
            .repo
            .session_step(session_id)
            .await
            .map_err(internal)?
            .filter(|step| step.owner == *actor)
            .ok_or(FactoryError::NotFound)?;
        Ok(SessionStepRef {
            run_id: step.key.run_id,
            seq: step.key.seq,
        })
    }

    #[tracing::instrument(skip(self, actor, request), fields(action = request.action().as_str()))]
    async fn steer_step(
        &self,
        actor: MacroUserIdStr<'static>,
        id: RunId,
        seq: u16,
        request: TerminalRequest,
    ) -> Result<(), FactoryError> {
        let steer = request.steer()?;
        self.known(&actor).await?;
        self.not_an_agent(&actor).await?;
        let run = self.owned_run(&actor, id).await?;
        if run.end.is_some() {
            return Err(FactoryError::RunEnded);
        }
        let key = StepKey { run_id: id, seq };
        // Kept first: the Step's work reads it while it waits on its agent,
        // so a pause or a stop holds the Step before the turn stops, and a
        // note or a resume is followed once its prompt is sent.
        let steered = match self.repo.steer(key, &steer).await.map_err(internal)? {
            Ok(steered) => steered,
            Err(SteerRefusal::Ended) => return Err(FactoryError::StepEnded),
            Err(refusal) => return Err(FactoryError::NotSteerable(refusal)),
        };
        let now = Utc::now();
        let (act, prompt_id) = match &steer {
            NewSteer::Note { note, prompt_id } => (
                SessionSteer::Prompt {
                    prompt_id: *prompt_id,
                    prompt: note_prompt(note, now),
                },
                Some(*prompt_id),
            ),
            NewSteer::Resume { note, prompt_id } => (
                SessionSteer::Prompt {
                    prompt_id: *prompt_id,
                    prompt: resume_prompt(note.as_deref(), now),
                },
                Some(*prompt_id),
            ),
            NewSteer::Pause | NewSteer::Stop => (SessionSteer::Interrupt, None),
        };
        let acted = self
            .executor
            .steer(&steered.role, key, &actor, &steered.session, act)
            .await;
        match (acted, prompt_id) {
            (Ok(()), _) => Ok(()),
            // The session refused the prompt: the agent never reads it, so
            // the Step no longer follows it.
            (Err(error), Some(prompt_id)) => {
                tracing::warn!(?error, "a step's session did not take its owner's prompt");
                self.repo
                    .unsteer(key, prompt_id, steered.previous_prompt)
                    .await
                    .map_err(internal)?;
                Err(FactoryError::SessionUnreachable)
            }
            // The turn may not have stopped, but the Step holds: its work
            // stops the turn again while it waits.
            (Err(error), None) => {
                tracing::warn!(?error, "a step's session was not stopped for its owner");
                Ok(())
            }
        }
    }

    #[tracing::instrument(skip(self, actor, reply))]
    async fn reply_to_gate(
        &self,
        actor: MacroUserIdStr<'static>,
        id: RunId,
        seq: u16,
        reply: String,
    ) -> Result<(), FactoryError> {
        let reply = reply.trim().to_string();
        if reply.is_empty() {
            return Err(FactoryError::AnswerMissing);
        }
        if reply.len() > MAX_NOTE_BYTES {
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

        // An agent's question takes the owner's words as its answer, as
        // the answer path does, on any worker; a Flow's Gate has them read.
        let steps = self.repo.list_steps(id).await.map_err(internal)?;
        let question = steps
            .iter()
            .find(|step| step.seq == seq)
            .and_then(|step| step.gate.as_ref())
            .is_some_and(|gate| gate.kind.answered_only_by_a_person());
        let delivered = if question {
            let answer = AnswerGate {
                seq,
                answer: Outcome::answer(),
                note: Some(reply),
                user_id: actor,
                approved_control_files: None,
                agent_session: None,
            };
            self.workflows.answer(id, &answer).await?
        } else {
            let reply = ReplyGate {
                seq,
                reply,
                user_id: actor,
            };
            self.workflows.reply(id, &reply).await?
        };
        match delivered {
            AnswerDelivery::Taken => Ok(()),
            AnswerDelivery::Refused(why) => Err(FactoryError::AnswerRefused(why)),
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
        answerer: Answerer,
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
        if let Answerer::Agent { session_id } = answerer {
            self.agent_may_answer(
                &run,
                &steps,
                seq,
                &answer,
                approved_control_files.is_some(),
                session_id,
            )
            .await?;
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
            agent_session: match answerer {
                Answerer::InPerson => None,
                Answerer::Agent { session_id } => Some(session_id),
            },
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
        // A team's Factory Definition holds its Control Files: these change
        // nothing while it names one.
        let named = self.named_definition(team).await?.is_some();
        Ok(ControlFilesSettings {
            is_default: own.is_none(),
            patterns: own.unwrap_or_else(ControlFiles::default_patterns),
            team_id: team.map(|team| team.team_id),
            can_edit: !named && team.is_some_and(|team| team.admin),
            definition_named: named,
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
        let team = self.settings_team(&actor).await?;
        self.repo
            .set_team_control_files(team.team_id, patterns.as_deref())
            .await
            .map_err(internal)?;
        self.control_files(&actor).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn definition(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<DefinitionSettings, FactoryError> {
        self.definition_settings(actor).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn set_definition(
        &self,
        actor: MacroUserIdStr<'static>,
        repo_url: Option<String>,
    ) -> Result<DefinitionSettings, FactoryError> {
        self.name_definition(actor, repo_url).await
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
        // A team's Factory Definition holds its rules: these change nothing
        // while it names one.
        let named = self.named_definition(team).await?.is_some();
        Ok(StandingRulesSettings {
            is_default: own.is_none(),
            definition: own.unwrap_or(OrchestratorDefinition::DEFAULT),
            projects,
            team_id: team.map(|team| team.team_id),
            can_edit: !agent && !named && team.is_some_and(|team| team.admin),
            definition_named: named,
        })
    }

    #[tracing::instrument(skip(self, actor))]
    async fn set_standing_rules(
        &self,
        actor: MacroUserIdStr<'static>,
        definition: Option<OrchestratorDefinition>,
    ) -> Result<StandingRulesSettings, FactoryError> {
        valid_queue_limit(definition.as_ref())?;
        let team = self.settings_team(&actor).await?;
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
        valid_queue_limit(definition.as_ref())?;
        let team = self.settings_team(&actor).await?;
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

    #[tracing::instrument(skip(self, actor, reason))]
    async fn mark_run_flawed(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
        reason: &str,
    ) -> Result<RunDetail, FactoryError> {
        self.owned_run(actor, id).await?;
        let by = FlawMarker::Person {
            user_id: actor.clone(),
        };
        mark_flawed(&self.repo, id, by, reason).await?;
        self.get_run(actor, id).await
    }
}

/// Fails when `definition` sets a Queue limit below [`MIN_QUEUE_LIMIT`] or
/// above [`MAX_QUEUE_LIMIT`].
fn valid_queue_limit(definition: Option<&OrchestratorDefinition>) -> Result<(), FactoryError> {
    let out_of_range = definition
        .and_then(|definition| definition.queue_limit)
        .is_some_and(|limit| !(MIN_QUEUE_LIMIT..=MAX_QUEUE_LIMIT).contains(&limit));
    if out_of_range {
        return Err(FactoryError::InvalidQueueLimit {
            min: MIN_QUEUE_LIMIT,
            max: MAX_QUEUE_LIMIT,
        });
    }
    Ok(())
}

/// How many questions an agent Step may ask the Run's owner, unless the
/// service is told otherwise: the next one ends the Run for a person.
pub const DEFAULT_QUESTIONS: u8 = 2;

/// What a Run's worker does for its workflow: run Steps, and record Steps,
/// Gates and the Run's end, each on its own. A Run whose owner section is
/// on has its waiting Gates told to its owner through `notifier`, and its
/// owner's comments on its task read through `notes`. A Run whose task
/// thread section is on has its updates posted to its task through
/// `thread`, once each is recorded.
#[derive(Debug, Clone)]
pub struct StepServiceImpl<R, E, N = NoOwnerGates, T = NoOwnerGates, P = NoTaskThread> {
    repo: R,
    executor: E,
    notifier: N,
    notes: T,
    thread: P,
}

impl<R, E> StepServiceImpl<R, E>
where
    R: RunRepository + ReviewRoundRepository + MainHoldRepository + EvaluationRepository,
    E: StepExecutor,
{
    /// Create the Step service, which tells no owner of a waiting Gate,
    /// reads no comment and posts no update, until
    /// [`Self::with_owner_gates`] and [`Self::with_task_thread`].
    pub fn new(repo: R, executor: E) -> Self {
        Self {
            repo,
            executor,
            notifier: NoOwnerGates,
            notes: NoOwnerGates,
            thread: NoTaskThread,
        }
    }
}

impl<R, E, N, T, P> StepServiceImpl<R, E, N, T, P>
where
    R: RunRepository + ReviewRoundRepository + MainHoldRepository + EvaluationRepository,
    E: StepExecutor,
    N: GateNotifier,
    T: TaskNotes,
    P: TaskThread,
{
    /// The service, telling Runs' owners of their waiting Gates through
    /// `notifier`, and reading their comments on the Runs' tasks through
    /// `notes`, for the Runs whose owner section is on.
    pub fn with_owner_gates<N2: GateNotifier, T2: TaskNotes>(
        self,
        notifier: N2,
        notes: T2,
    ) -> StepServiceImpl<R, E, N2, T2, P> {
        StepServiceImpl {
            repo: self.repo,
            executor: self.executor,
            notifier,
            notes,
            thread: self.thread,
        }
    }

    /// The service, posting Runs' updates to their tasks through `thread`,
    /// for the Runs whose task thread section is on.
    pub fn with_task_thread<P2: TaskThread>(self, thread: P2) -> StepServiceImpl<R, E, N, T, P2> {
        StepServiceImpl {
            repo: self.repo,
            executor: self.executor,
            notifier: self.notifier,
            notes: self.notes,
            thread,
        }
    }

    /// Post `update` of the Run `run_id` to its task's thread, once what it
    /// tells is recorded. It never fails the activity that posts it.
    async fn post(&self, run_id: RunId, update: RunUpdate) {
        post_update(&self.repo, &self.thread, run_id, update).await;
    }

    /// What the owner of `run` commented on its task since the latest Gate
    /// of the Steps `earlier` opened: none unless the Run's owner section
    /// is on, it works on a task, and a Gate opened. Comments that cannot
    /// be read now fail the Step's attempt, to be tried again.
    async fn owner_notes(&self, run: &Run, earlier: &[Step]) -> Result<Vec<OwnerNote>, StepError> {
        let owner_gates = run
            .standing_rules
            .is_some_and(|rules| rules.definition.owner_gates);
        if !owner_gates {
            return Ok(Vec::new());
        }
        let (Some(task_id), Some(since)) = (run.task_id, notes_since(earlier)) else {
            return Ok(Vec::new());
        };
        // Comments the factory posted as the owner are not their notes.
        let notes = self
            .notes
            .owner_notes(task_id, &run.user_id, since)
            .await
            .map_err(|error| StepError::Internal(internal(error)))?;
        Ok(notes
            .into_iter()
            .filter(|note| !is_factory_comment(&note.text))
            .collect())
    }

    async fn run_of(&self, request: &StepRequest) -> Result<Run, StepError> {
        self.repo
            .get_run(request.key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)
    }
}

impl<R, E, N, T, P> StepService for StepServiceImpl<R, E, N, T, P>
where
    R: RunRepository + ReviewRoundRepository + MainHoldRepository + EvaluationRepository,
    E: StepExecutor,
    N: GateNotifier,
    T: TaskNotes,
    P: TaskThread,
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
        // The Run's first Step starting is the Run starting.
        if request.key.seq == 1 {
            self.post(request.key.run_id, RunUpdate::Started).await;
        }
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
        let owner_notes = self.owner_notes(&run, &earlier).await?;
        // A Benchmark trial's branch starts from the commit its suite
        // pinned, so every trial of a task replays the same code.
        let base_commit = self
            .repo
            .trial_base_commit(request.key.run_id)
            .await
            .map_err(internal)?;
        let input = StepInput {
            key: request.key,
            role: request.role.as_ref(),
            owner: &run.user_id,
            request: &run.request,
            created_in_person: run.created_in_person,
            control_file_patterns: run.control_file_patterns.as_deref(),
            definition_repo: run
                .definition
                .as_ref()
                .map(|definition| definition.repo_url.as_str()),
            task_id: run.task_id,
            repository: run.repository.as_ref(),
            earlier: &earlier,
            review,
            reviewer: request.round.is_some(),
            may_ask: request.may_ask,
            questions_left: request.questions_left,
            resumes: request.resumes,
            owner_notes: &owner_notes,
            base_commit: base_commit.as_deref(),
        };
        let report = self
            .executor
            .run(&request.step, input)
            .await
            .map_err(|error| {
                let out_of_quota = error.out_of_quota();
                let overdue = error.overdue();
                let final_failure = !error.retryable();
                let report = internal(error);
                if out_of_quota {
                    StepError::OutOfQuota(report)
                } else if overdue {
                    StepError::Overdue(report)
                } else if final_failure {
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
        // Work its owner stopped, or that stayed paused past its time,
        // fails for good: the Step says why rather than the workflow's
        // word for any failed work.
        let mut output = ending.report.output;
        if ending.report.outcome == Outcome::failed() {
            let steering = self.repo.steering(ending.key).await.map_err(internal)?;
            if steering.pause_ran_out {
                output = PAUSE_RAN_OUT_OUTPUT.to_string();
            } else if steering.stop_asked {
                output = STOPPED_OUTPUT.to_string();
            }
        }
        self.repo
            .end_step(ending.key, &ending.report.outcome, &output)
            .await
            .map_err(internal)?;
        let ended = RunUpdate::StepEnded {
            seq: ending.key.seq,
        };
        self.post(ending.key.run_id, ended).await;
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
                answer.agent_session,
            )
            .await
            .map_err(internal)?;

        // A Gate its owner was told of is answered: its notification is
        // done, whoever answered.
        let run = self
            .repo
            .get_run(answered.key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        let owner_gates = run
            .standing_rules
            .is_some_and(|rules| rules.definition.owner_gates);
        if owner_gates {
            let id = notice_id(answered.key.run_id, answered.key.seq);
            self.notifier.answered(&run.user_id, id).await;
        }
        // An answer in the stand-in's window closes it, and so the notice
        // that the stand-in answered.
        let stood_in = self
            .repo
            .list_steps(answered.key.run_id)
            .await
            .map_err(internal)?
            .into_iter()
            .find(|step| step.seq == answered.key.seq)
            .and_then(|step| step.gate)
            .is_some_and(|gate| gate.stand_in.is_some());
        if stood_in {
            let id = stand_in_id(answered.key.run_id, answered.key.seq);
            self.notifier.answered(&run.user_id, id).await;
        }
        let update = RunUpdate::GateAnswered {
            seq: answered.key.seq,
        };
        self.post(answered.key.run_id, update).await;
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
            let ruled = answer.rule.map(|rule| RuleAnswer {
                rule,
                answer: answer.answer.clone(),
            });
            // An attempt that answered before its result was lost may not
            // have posted it: posting it again posts it once.
            if ruled.is_some() {
                self.post(key.run_id, RunUpdate::GateAnswered { seq: key.seq })
                    .await;
            }
            return Ok(ruled);
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
        // Whether a review round found its round before's Blockers again
        // decides only with the owner section on.
        let repeated = match ruling.kind {
            RuleGateKind::Blockers if rules.definition.owner_gates => {
                let rounds = self
                    .repo
                    .review_rounds(key.run_id)
                    .await
                    .map_err(internal)?;
                repeated_blockers(&rounds, &steps, key.seq)
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
            repeated_blockers: repeated,
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
        self.post(key.run_id, RunUpdate::GateAnswered { seq: key.seq })
            .await;
        Ok(Some(RuleAnswer {
            rule,
            answer: kept.answer,
        }))
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %key.run_id, seq = key.seq))]
    async fn notify_gate(&self, key: StepKey) -> Result<(), StepError> {
        let run = self
            .repo
            .get_run(key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        let steps = self.repo.list_steps(key.run_id).await.map_err(internal)?;
        let notice = steps
            .iter()
            .find(|step| step.seq == key.seq)
            .and_then(|step| gate_notice(&run, step));
        if let Some(notice) = notice {
            self.notifier.notify(notice).await;
        }
        self.post(key.run_id, RunUpdate::GateWaiting { seq: key.seq })
            .await;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %telling.key.run_id, seq = telling.key.seq))]
    async fn tell_gate(&self, telling: GateTelling) -> Result<(), StepError> {
        let key = telling.key;
        let run = self
            .repo
            .get_run(key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        let steps = self.repo.list_steps(key.run_id).await.map_err(internal)?;
        let told = steps
            .iter()
            .find(|step| step.seq == key.seq)
            .and_then(|step| gate_told(&run, step, &telling.question));
        if let Some(notice) = told {
            self.notifier.notify(notice).await;
        }
        let told = RunUpdate::GateTold {
            seq: key.seq,
            question: telling.question,
        };
        self.post(key.run_id, told).await;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %finish.run_id, outcome = %finish.outcome))]
    async fn end_run(&self, finish: RunFinish) -> Result<(), StepError> {
        self.repo
            .end_run(finish.run_id, finish.outcome)
            .await
            .map_err(internal)?;

        // A Gate left open when the Run ended (cancelled, or ended for a
        // person) waits on no one now: the notification its owner was told
        // of it by is done, as an answer would make it. One never sent
        // changes nothing, so a retry marks the same ones again.
        let run = self
            .repo
            .get_run(finish.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        let owner_gates = run
            .standing_rules
            .is_some_and(|rules| rules.definition.owner_gates);
        let steps = self
            .repo
            .list_steps(finish.run_id)
            .await
            .map_err(internal)?;
        if owner_gates {
            for step in &steps {
                if step.gate.as_ref().is_some_and(|gate| gate.answer.is_none()) {
                    let id = notice_id(finish.run_id, step.seq);
                    self.notifier.answered(&run.user_id, id).await;
                }
            }
        }

        // A Run that ends in a stand-in's window closes it: the notice
        // that the stand-in answered is done, as the window closing would
        // make it.
        for step in &steps {
            let in_window = step
                .gate
                .as_ref()
                .is_some_and(|gate| gate.answer.is_none() && gate.stand_in.is_some());
            if in_window {
                let id = stand_in_id(finish.run_id, step.seq);
                self.notifier.answered(&run.user_id, id).await;
            }
        }

        // A notify-only Gate's notice is news of a Run under way: once the
        // Run ends it is done. No record says which entries told, and
        // marking a notice never sent changes nothing, so each entry's is
        // marked, whatever the owner section says, as the notice was sent.
        let entries: BTreeSet<_> = steps.iter().map(|step| &step.entry).collect();
        for entry in entries {
            let id = told_id(finish.run_id, entry);
            self.notifier.answered(&run.user_id, id).await;
        }
        self.post(finish.run_id, RunUpdate::Ended).await;
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
