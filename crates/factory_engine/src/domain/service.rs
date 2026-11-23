//! The factory's use cases: starting and reading Runs, and running their
//! Steps.

mod board;
mod definition;
pub(crate) mod quarantine;
pub(crate) mod retry;
mod standing;
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
use factory_records::domain::review_rounds::{
    BlockerResolutions, ReviewRound, ReviewRoundRepository,
};
use factory_records::domain::self_improvement::{IMPROVE_FLOW, SelfImprovementRepository};
use factory_records::domain::standing_acts::{StandingAct, StandingActRepository, StandingActor};
use factory_records::domain::standing_rules::{
    MAX_QUEUE_LIMIT, MAX_TOKEN_BUDGET, MIN_QUEUE_LIMIT, MIN_TOKEN_BUDGET, OrchestratorDefinition,
    ProjectRulesSet, RuleGateKind, RulesSource, RunRules, StandingRulesRepository,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::agents::{AgentDefinition, RuleSubject};
use crate::domain::board::{BoardFilter, BoardPage, PullRequestRuns};
use crate::domain::built_in_flows::{BUILT_IN_FLOWS, BuiltInFlow, walked_flow};
use crate::domain::control_files::ControlFiles;
use crate::domain::definition::{DEFINITION_FILE_PATTERNS, DefinitionError};
use crate::domain::evaluation::{EvaluationDefinition, TeamEvaluation};
use crate::domain::flaws::mark_flawed;
use crate::domain::flow::{Entry, Flow, StepName, StepUse};
use crate::domain::gate::{
    AnswerGate, GateAnswered, GateOpening, GateRuling, GateTelling, ReplyGate, StandingAnswer,
};
use crate::domain::gate_owner::{
    OwnerNote, gate_notice, gate_told, is_factory_comment, notes_since, notice_id,
    notice_may_be_out, stand_in_id, stand_in_notice_may_be_out, told_entries, told_id,
};
use crate::domain::models::{
    ActingAgent, Answerer, CodeRepository, ControlFilesSettings, DefinitionSettings, FactoryError,
    FlowSummary, PersonOnly, RunDetail, RunFinish, RunStart, SessionSteer, SessionStepRef,
    StandingRulesSettings, StepEnding, StepError, StepInput, StepReport, StepRequest, StepStart,
};
use crate::domain::plan_review::blockers_left;
use crate::domain::ports::{
    AgentFailure, AnswerDelivery, FactoryService, GateNotifier, NoOwnerGates, NoQuarantineTasks,
    NoTaskThread, PullRequests, QuarantineTasks, Retryable, RunWorkflows, RunWorkflowsError,
    StepExecutor, StepService, TaskAccess, TaskDirectory, TaskNotes, TaskThread, UserDirectory,
};
use crate::domain::standing_rules::{
    GateFacts, RuleAnswer, any_dismissed, kind_answers, offers_kind_answers, repeated_blockers,
    rule_answer, rule_fix_limit,
};
use crate::domain::step_terminal::{
    PAUSE_RAN_OUT_OUTPUT, STOPPED_OUTPUT, TerminalRequest, note_prompt, paused_step_ends_by,
    resume_prompt,
};
use crate::domain::task_thread::{RunUpdate, post_update};
use crate::domain::token_budget::{
    BudgetCheck, BudgetVerdict, RunTokenBudget, RunTokens, UsageLog, budget_notice, budget_of,
    verdict,
};
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

/// How many times a cancel's standing act is tried before who sent the
/// cancel is given up as lost.
const CANCEL_RECORD_ATTEMPTS: u8 = 3;

/// The wait before the second try of a cancel's standing act, and the
/// step it grows by for each later one.
const CANCEL_RECORD_BACKOFF: std::time::Duration = std::time::Duration::from_millis(200);

/// A Run to record and start: what [`FactoryService::create_run`] takes,
/// and the Automation that starts it, if one does.
struct RunOrder<'a> {
    actor: MacroUserIdStr<'static>,
    flow: String,
    request: String,
    task_id: Option<Uuid>,
    repo_url: Option<String>,
    in_person: bool,
    /// Who starts the Run for its owner, and whether Standing Roles were
    /// checked for it already.
    starter: Starter,
    automation: Option<AutomationStart<'a>>,
    /// The Benchmark trial it is, if a suite starts it, and the commit its
    /// write starts from.
    trial: Option<(TrialKey, String)>,
    /// The self-improvement Run it is, kept as starting, if the factory
    /// starts it against its team's Definition.
    improvement: Option<Uuid>,
}

/// Who starts a Run, for Standing Roles' `start`.
enum Starter {
    /// The agent that starts it for its owner, if one does: checked as
    /// the Run starts.
    Acting(Option<ActingAgent>),
    /// The agent a suite's check let start its trials, if one did,
    /// checked once for the suite before it kept anything.
    Checked(Option<StandingActor>),
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
    usage: Option<UsageLog>,
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
        + EvaluationRepository
        + StandingActRepository
        + SelfImprovementRepository,
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
            usage: None,
        }
    }

    /// Read what a Run's agent Steps used of its token budget from `usage`,
    /// for the Run API; without it, a read Run's use reads as unknown.
    pub fn with_usage(mut self, usage: impl RunTokens + 'static) -> Self {
        self.usage = Some(UsageLog(Arc::new(usage)));
        self
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

    /// The tags of the task `task_id` as `user_id` sees them, for the
    /// Agent Rules of the Definition a Run starts under; none when they
    /// cannot be read. Then no rule on a tag matches and the Role's own
    /// order applies, as for a task with no tags, and the Run keeps that
    /// its tags went unread; the Run does not wait on document storage.
    async fn run_task_tags(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Option<Vec<String>> {
        match self.tasks.task_tags(user_id, task_id).await {
            Ok(tags) => Some(tags),
            Err(error) => {
                tracing::warn!(
                    ?error,
                    %task_id,
                    "the run task's tags could not be read: no agent rule on a tag matches it"
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
        let standing_acts = self
            .repo
            .runs_standing_acts(&[run.id])
            .await
            .map_err(internal)?
            .remove(&run.id)
            .unwrap_or_default();
        let agent_rules = self
            .repo
            .runs_agent_rules(&[run.id])
            .await
            .map_err(internal)?
            .remove(&run.id)
            .unwrap_or_default();
        let task_tags_unread = !self
            .repo
            .runs_task_tags_unread(&[run.id])
            .await
            .map_err(internal)?
            .is_empty();
        let token_budget = self.token_budget(&run).await?;
        let break_records = self
            .repo
            .runs_break_records(&[run.id])
            .await
            .map_err(internal)?
            .remove(&run.id)
            .unwrap_or_default();
        Ok(RunDetail {
            run,
            steps: with_pause_ends(steps),
            main_hold: base.hold,
            base_required_checks: base.required,
            blocker_resolutions: BlockerResolutions::of_rounds(&review_rounds),
            review_rounds,
            standing_acts,
            agent_rules,
            task_tags_unread,
            token_budget,
            break_records,
        })
    }

    /// The token budget `run` keeps, with what its agent Steps used of it
    /// so far, read from the usage log: unknown when it cannot be read,
    /// which the Run's read survives.
    async fn token_budget(&self, run: &Run) -> Result<Option<RunTokenBudget>, FactoryError> {
        let Some(budget) = budget_of(run) else {
            return Ok(None);
        };
        let used = match &self.usage {
            Some(usage) => {
                let sessions = self.repo.step_sessions(run.id).await.map_err(internal)?;
                match usage.0.used(&run.user_id, &sessions).await {
                    Ok(used) => Some(used),
                    Err(error) => {
                        tracing::warn!(?error, run_id = %run.id, "a run's token use was not read");
                        None
                    }
                }
            }
            None => None,
        };
        Ok(Some(RunTokenBudget {
            budget,
            used: used.map(|used| used.unwrap_or(0)),
            measured: !matches!(used, Some(None)),
            stop: run.budget_stop,
        }))
    }

    /// `runs`, each with its Steps, its review rounds, and what was read of
    /// its base branch, in their order.
    async fn details(&self, runs: Vec<Run>) -> Result<Vec<RunDetail>, FactoryError> {
        let ids: Vec<RunId> = runs.iter().map(|run| run.id).collect();
        let mut steps = self.repo.list_runs_steps(&ids).await.map_err(internal)?;
        let mut rounds = self.repo.runs_review_rounds(&ids).await.map_err(internal)?;
        let mut standing_acts = self.repo.runs_standing_acts(&ids).await.map_err(internal)?;
        let mut agent_rules = self.repo.runs_agent_rules(&ids).await.map_err(internal)?;
        let tags_unread = self
            .repo
            .runs_task_tags_unread(&ids)
            .await
            .map_err(internal)?;
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
            let standing_acts = standing_acts.remove(&run.id).unwrap_or_default();
            let agent_rules = agent_rules.remove(&run.id).unwrap_or_default();
            let task_tags_unread = tags_unread.contains(&run.id);
            // Break records are read for one Run's detail only.
            let break_records = Vec::new();
            // A list shows each Run's budget, not its use: reading the
            // usage log for every Run listed would cost a read each.
            let token_budget = budget_of(&run).map(|budget| RunTokenBudget {
                budget,
                used: None,
                measured: true,
                stop: run.budget_stop,
            });
            details.push(RunDetail {
                run,
                steps,
                main_hold: base.hold,
                base_required_checks: base.required,
                blocker_resolutions: BlockerResolutions::of_rounds(&review_rounds),
                review_rounds,
                standing_acts,
                agent_rules,
                task_tags_unread,
                token_budget,
                break_records,
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
    /// to a fix the `fix_blockers` rule would leave to the owner. Gives
    /// the kind of the Gate when it may answer it.
    async fn agent_may_answer(
        &self,
        run: &Run,
        steps: &[Step],
        seq: u16,
        answer: &Outcome,
        approved_control_files: bool,
        session_id: Uuid,
    ) -> Result<RuleGateKind, FactoryError> {
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
            return Ok(kind);
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
        Ok(kind)
    }

    /// Cancel the workflow of the Run `id`, which has not ended: fails
    /// when the workflow ended it before the cancel reached it.
    async fn send_cancel(&self, id: RunId) -> Result<(), FactoryError> {
        if self.workflows.cancel(id).await? {
            return Ok(());
        }
        // Temporal had no running workflow for the Run. One may have
        // started since, which a second cancel reaches; one that completed
        // before its Run's end was recorded has ended the Run, though the
        // records don't say so yet.
        if self.workflows.exists(id).await? {
            return if self.workflows.cancel(id).await? {
                Ok(())
            } else {
                Err(FactoryError::RunEnded)
            };
        }
        // A Run whose workflow never started has nothing to stop: it ends
        // cancelled here. A workflow that starts late takes no Step of it,
        // and is cancelled if it is there by now.
        self.repo
            .end_run(id, RunOutcome::Cancelled)
            .await
            .map_err(internal)?;
        if self.workflows.exists(id).await? {
            self.workflows.cancel(id).await?;
        }
        Ok(())
    }

    /// Keep `actor` as who cancelled the Run `id`, once its cancel was
    /// taken. The cancel cannot be taken back, and a caller's retry finds
    /// the Run cancelled, which keeps no one, so a failed record is tried
    /// again here, `CANCEL_RECORD_ATTEMPTS` times in all, and one that
    /// still fails is logged as lost before its error is answered.
    async fn record_cancel(&self, id: RunId, actor: &StandingActor) -> Result<(), FactoryError> {
        let mut attempt: u8 = 1;
        loop {
            match self
                .repo
                .record_standing_act(id, StandingAct::Cancel, None, actor)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) if attempt < CANCEL_RECORD_ATTEMPTS => {
                    tracing::warn!(
                        ?error,
                        run_id = %id,
                        attempt,
                        "recording who cancelled a run failed; trying again"
                    );
                    tokio::time::sleep(CANCEL_RECORD_BACKOFF * u32::from(attempt)).await;
                    attempt += 1;
                }
                Err(error) => {
                    tracing::error!(
                        ?error,
                        run_id = %id,
                        bot_id = %actor.bot_id,
                        session_id = ?actor.session_id,
                        standing_role = %actor.standing_role,
                        "a run's cancel was taken but who sent it was not recorded"
                    );
                    return Err(internal(error).into());
                }
            }
        }
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
            starter,
            automation,
            trial,
            improvement,
        } = order;
        // A Benchmark's Flow runs only as a suite's trial, and a trial only
        // on it; the improve Flow only as a self-improvement Run, and that
        // only on it.
        if (flow == BENCHMARK_FLOW) != trial.is_some()
            || (flow == IMPROVE_FLOW) != improvement.is_some()
        {
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
        // So does a self-improvement Run walk the built-in improve Flow,
        // on the Definition's own repository, whose Agents must fill it.
        let flows_of = definition
            .as_ref()
            .filter(|_| trial.is_none() && improvement.is_none());
        let flow = self.flow_named(&flow_name, flows_of)?;
        if improvement.is_some() {
            let Some(pinned) = &definition else {
                return Err(FactoryError::UnknownFlow);
            };
            let on_definition = repo_url
                .as_deref()
                .is_some_and(|repo_url| pinned.repo_url.eq_ignore_ascii_case(repo_url));
            if !on_definition {
                return Err(FactoryError::InvalidRepository);
            }
            let roles: BTreeSet<_> = flow
                .entries
                .iter()
                .flat_map(|entry| entry.role.iter().chain(&entry.reviewers))
                .collect();
            pinned.definition.agents.fills(roles).map_err(|error| {
                FactoryError::InvalidDefinition(DefinitionError::ImprovementAgents(error))
            })?;
        }
        // An agent starts a Run under a Definition with Standing Roles only
        // when its bot fills one that allows it.
        let started_by = match starter {
            Starter::Acting(acting) => {
                self.standing_actor(
                    &actor,
                    definition.as_ref().map(|pinned| &pinned.definition.agents),
                    StandingAct::Start,
                    acting,
                )
                .await?
            }
            Starter::Checked(started_by) => started_by,
        };
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
        // The tags its Definition's Agent Rules match, read once as it
        // starts: the rules it keeps say which chose its Agents, so a tag
        // changed later does not change a Run under way.
        let task_tags = match (&definition, task_id) {
            (Some(pinned), Some(task_id)) if pinned.definition.agents.has_tag_rules() => {
                self.run_task_tags(&actor, task_id).await
            }
            _ => Some(Vec::new()),
        };
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

        // The Run keeps its Definition's known flaky CI jobs before its
        // workflow starts, as it keeps its rules: a change merged later
        // does not change which jobs its `ci` Steps rerun.
        let known_flakes = definition
            .as_ref()
            .map(|pinned| pinned.definition.rules.known_flakes.as_slice())
            .unwrap_or_default();
        if !known_flakes.is_empty()
            && let Err(error) = self.repo.keep_known_ci_flakes(run_id, known_flakes).await
        {
            self.forget_unstarted(run_id).await;
            return Err(FactoryError::Internal(internal(error)));
        }

        // The Run keeps its trial, or the arms of its Flow's Experiments,
        // before its workflow starts, so its first Step already runs on
        // their Agents. A Run that cannot keep them does not start. A
        // team's Definition holds its Experiments; a team with none takes
        // the deployment's.
        let kept = match (&trial, improvement) {
            (Some((trial, base_commit)), _) => {
                self.repo.insert_trial(trial, run_id, base_commit).await
            }
            // A self-improvement Run is kept as its improvement's before
            // its workflow starts; one no longer starting does not start.
            (None, Some(improvement)) => {
                match self.repo.link_improvement_run(improvement, run_id).await {
                    Ok(true) => Ok(()),
                    Ok(false) => {
                        self.forget_unstarted(run_id).await;
                        return Err(FactoryError::Internal(rootcause::report!(
                            "the self-improvement run is no longer starting"
                        )));
                    }
                    Err(error) => {
                        self.forget_unstarted(run_id).await;
                        return Err(FactoryError::Internal(internal(error)));
                    }
                }
            }
            (None, None) => {
                let evaluation = match &definition {
                    Some(pinned) => &pinned.definition.evaluation,
                    None => &*self.evaluation,
                };
                let arms = evaluation.assign_arms(&flow_name, task_id);
                match self.repo.record_arms(run_id, &arms).await {
                    // Its Definition's Agent Rules choose the first Agent
                    // of each Role no arm chose for: an arm's choice comes
                    // first, so an Experiment compares what it set out to.
                    Ok(()) => match &definition {
                        Some(pinned) => {
                            let subject = RuleSubject {
                                tags: task_tags.as_deref().unwrap_or_default(),
                                repository: repository.as_ref().map(|kept| kept.url.as_str()),
                            };
                            let rules = pinned.definition.agents.chosen_by_rules(subject, |role| {
                                arms.iter().any(|arm| arm.agents.contains_key(role))
                            });
                            match self.repo.record_agent_rules(run_id, &rules).await {
                                // The Run API says the tags went unread, so
                                // no rule on a tag matching reads apart from
                                // a task no rule matched.
                                Ok(()) if task_tags.is_none() => {
                                    self.repo.record_task_tags_unread(run_id).await
                                }
                                kept => kept,
                            }
                        }
                        None => Ok(()),
                    },
                    Err(error) => Err(error),
                }
            }
        };
        if let Err(error) = kept {
            self.forget_unstarted(run_id).await;
            return Err(FactoryError::Internal(internal(error)));
        }

        // The agent that started it is kept before its workflow starts: a
        // Run whose start could not be kept is removed, as one whose
        // workflow did not start.
        if let Some(started_by) = &started_by
            && let Err(error) = self
                .repo
                .record_standing_act(run_id, StandingAct::Start, None, started_by)
                .await
        {
            self.forget_unstarted(run_id).await;
            return Err(internal(error).into());
        }

        // The Run is recorded before its workflow starts, so the workflow can
        // record its Steps against it.
        let start = RunStart {
            run_id,
            flow,
            token_budget: budget_of(&run).is_some(),
            start_at: None,
        };
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
        + EvaluationRepository
        + StandingActRepository
        + SelfImprovementRepository,
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
            .filter(|flow| flow.name != BENCHMARK_FLOW && flow.name != IMPROVE_FLOW)
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
        acting: Option<ActingAgent>,
    ) -> Result<RunDetail, FactoryError> {
        self.start_run(RunOrder {
            actor,
            flow,
            request,
            task_id,
            repo_url,
            in_person,
            starter: Starter::Acting(acting),
            automation: None,
            trial: None,
            improvement: None,
        })
        .await
    }

    async fn standing_start(
        &self,
        actor: &MacroUserIdStr<'static>,
        agents: Option<&AgentDefinition>,
        acting: Option<ActingAgent>,
    ) -> Result<Option<StandingActor>, FactoryError> {
        self.standing_actor(actor, agents, StandingAct::Start, acting)
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
        started_by: Option<StandingActor>,
    ) -> Result<RunDetail, FactoryError> {
        self.start_run(RunOrder {
            actor,
            flow,
            request,
            task_id: None,
            repo_url: Some(repo_url),
            in_person: false,
            starter: Starter::Checked(started_by),
            automation: None,
            trial: Some((trial, base_commit)),
            improvement: None,
        })
        .await
    }

    async fn create_improvement_run(
        &self,
        actor: MacroUserIdStr<'static>,
        request: String,
        repo_url: String,
        improvement: Uuid,
    ) -> Result<RunDetail, FactoryError> {
        self.start_run(RunOrder {
            actor,
            flow: IMPROVE_FLOW.to_string(),
            request,
            task_id: None,
            repo_url: Some(repo_url),
            in_person: false,
            starter: Starter::Acting(None),
            automation: None,
            trial: None,
            improvement: Some(improvement),
        })
        .await
    }

    async fn check_run_order(
        &self,
        actor: &MacroUserIdStr<'static>,
        flow: &str,
        repo_url: Option<&str>,
    ) -> Result<(), FactoryError> {
        if flow == BENCHMARK_FLOW || flow == IMPROVE_FLOW {
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
        acting: Option<ActingAgent>,
    ) -> Result<RunDetail, FactoryError> {
        let started_by = RunAutomation {
            id: automation.id,
            name: automation.name.clone(),
            trigger: automation.trigger.kind(),
        };
        // An Automation acts as its creator, never in their signed-in
        // session: its request names no Control File. An agent that fires
        // it starts its Run as any Run it starts, under Standing Roles.
        self.start_run(RunOrder {
            actor: automation.created_by.clone(),
            flow: automation.flow.clone(),
            request,
            task_id,
            repo_url: automation.repo_url.clone(),
            in_person: false,
            starter: Starter::Acting(acting),
            automation: Some(AutomationStart {
                automation: &started_by,
                firing_key,
            }),
            trial: None,
            improvement: None,
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
        acting: Option<ActingAgent>,
    ) -> Result<RunDetail, FactoryError> {
        self.owned_run(actor, id).await?;
        let agents = self.acting_agents(id, acting).await?;
        let linked_by = self
            .standing_actor(actor, agents.as_ref(), StandingAct::Link, acting)
            .await?;
        self.visible_task(actor, task_id).await?;

        match self.repo.link_task(id, task_id).await.map_err(internal)? {
            TaskLink::Linked => {
                // Linking again is safe: a failed record is kept by the
                // caller's retry.
                if let Some(linked_by) = &linked_by {
                    self.repo
                        .record_standing_act(id, StandingAct::Link, None, linked_by)
                        .await
                        .map_err(internal)?;
                }
                self.get_run(actor, id).await
            }
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
                standing: None,
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
        let mut standing = None;
        if let Answerer::Agent { session_id } = answerer {
            let kind = self
                .agent_may_answer(
                    &run,
                    &steps,
                    seq,
                    &answer,
                    approved_control_files.is_some(),
                    session_id,
                )
                .await?;
            // Under a Definition with Standing Roles, the agent's bot must
            // fill one that allows answering this kind of Gate. No act
            // answers a merge Gate, whatever the answer.
            let agents = self.kept_agents(id).await?;
            let act = match kind {
                RuleGateKind::Plan => Some(StandingAct::AnswerPlan),
                RuleGateKind::Blockers => Some(StandingAct::AnswerBlockers),
                RuleGateKind::Merge => None,
            };
            match act {
                Some(act) => {
                    standing = self
                        .standing_actor(
                            &actor,
                            agents.as_ref(),
                            act,
                            Some(ActingAgent::Session(session_id)),
                        )
                        .await?
                        .map(|by| StandingAnswer { act, by });
                }
                None if agents
                    .as_ref()
                    .is_some_and(AgentDefinition::has_standing_roles) =>
                {
                    return Err(PersonOnly::Merge.into());
                }
                None => {}
            }
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
            standing,
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

    #[tracing::instrument(skip(self, actor, change))]
    async fn propose_definition_change(
        &self,
        actor: MacroUserIdStr<'static>,
        change_id: Uuid,
        change: crate::domain::definition::ProposedChange,
    ) -> Result<crate::domain::models::DefinitionPullRequest, FactoryError> {
        self.propose_change(actor, change_id, change).await
    }

    #[tracing::instrument(skip(self, actor))]
    async fn definition_file(
        &self,
        actor: &MacroUserIdStr<'static>,
        path: String,
    ) -> Result<crate::domain::models::DefinitionFile, FactoryError> {
        self.read_definition_file(actor, path).await
    }

    #[tracing::instrument(skip(self, actor, change))]
    async fn check_definition_change(
        &self,
        actor: MacroUserIdStr<'static>,
        change: crate::domain::definition::ProposedChange,
    ) -> Result<crate::domain::models::DefinitionChangeCheck, FactoryError> {
        self.check_change(actor, change).await
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
        valid_token_budget(definition.as_ref())?;
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
        valid_token_budget(definition.as_ref())?;
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
        acting: Option<ActingAgent>,
        in_person: bool,
    ) -> Result<(), FactoryError> {
        let run = self.owned_run(actor, id).await?;
        let agents = self.acting_agents(id, acting).await?;
        let cancelled_by = self
            .standing_actor(actor, agents.as_ref(), StandingAct::Cancel, acting)
            .await?;
        match run.end.map(|end| end.outcome) {
            None => {
                // Whether the owner cancelled it in person is kept before the
                // cancel is sent, so its end, which may come at once, reads
                // it: only a person's cancel rejects a self-improvement Run.
                // An agent's account is never its owner in person.
                let in_person = in_person
                    && acting.is_none()
                    && !self.repo.is_agent_account(actor).await.map_err(internal)?;
                self.repo
                    .record_cancel_sender(id, in_person)
                    .await
                    .map_err(internal)?;
                self.send_cancel(id).await?;
                // The agent that asked for the cancel is kept once the
                // cancel is accepted, so a refused or failed one names no
                // one; a Run keeps the first cancel's.
                if let Some(cancelled_by) = &cancelled_by {
                    self.record_cancel(id, cancelled_by).await?;
                }
                Ok(())
            }
            // Cancelling again is what the first cancel already did.
            Some(RunOutcome::Cancelled) => Ok(()),
            Some(_) => Err(FactoryError::RunEnded),
        }
    }

    async fn retry_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        id: RunId,
        acting: Option<ActingAgent>,
        in_person: bool,
    ) -> Result<RunDetail, FactoryError> {
        self.retry(actor, id, acting, in_person).await
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
        // The flaw is a failure the team's Definition may learn from; a
        // failure to keep it is logged, and the mark stands.
        if let Err(error) = crate::domain::self_improvement::collect(&self.repo, id).await {
            tracing::warn!(?error, run_id = %id, "a flawed run's failure was not kept");
        }
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

/// Fails when `definition` sets a token budget below [`MIN_TOKEN_BUDGET`]
/// or above [`MAX_TOKEN_BUDGET`].
fn valid_token_budget(definition: Option<&OrchestratorDefinition>) -> Result<(), FactoryError> {
    let out_of_range = definition
        .and_then(|definition| definition.token_budget)
        .is_some_and(|budget| !(MIN_TOKEN_BUDGET..=MAX_TOKEN_BUDGET).contains(&budget));
    if out_of_range {
        return Err(FactoryError::InvalidTokenBudget {
            min: MIN_TOKEN_BUDGET,
            max: MAX_TOKEN_BUDGET,
        });
    }
    Ok(())
}

/// The most visits of the entry `entry` of the Flow a Run walks, which runs
/// `step`: its own bound, none when it has none; or, when the Run's Flow is
/// unknown (a Run that kept none and names no built-in Flow, or whose Flow
/// does not read), the built-in `code` Flow's bound of its entry of `step`,
/// as such Runs were bounded before Runs kept their Flows.
pub(crate) fn visits_bound(entry: Option<&Entry>, step: &StepName) -> Option<u16> {
    let bound = |entry: &Entry| entry.max_visits.map(|most| u16::from(most.get()));
    match entry {
        Some(entry) => bound(entry),
        None => BuiltInFlow::named("code")?
            .read(|_| Some(StepUse::Work))
            .ok()?
            .entries
            .iter()
            .find(|entry| entry.step == *step)
            .and_then(bound),
    }
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
pub struct StepServiceImpl<
    R,
    E,
    N = NoOwnerGates,
    T = NoOwnerGates,
    P = NoTaskThread,
    Q = NoQuarantineTasks,
> {
    repo: R,
    executor: E,
    notifier: N,
    notes: T,
    thread: P,
    quarantine_tasks: Q,
    usage: Option<UsageLog>,
}

impl<R, E> StepServiceImpl<R, E>
where
    R: RunRepository
        + ReviewRoundRepository
        + MainHoldRepository
        + EvaluationRepository
        + StandingActRepository,
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
            quarantine_tasks: NoQuarantineTasks,
            usage: None,
        }
    }
}

impl<R, E, N, T, P, Q> StepServiceImpl<R, E, N, T, P, Q>
where
    R: RunRepository
        + ReviewRoundRepository
        + MainHoldRepository
        + EvaluationRepository
        + StandingActRepository,
    E: StepExecutor,
    N: GateNotifier,
    T: TaskNotes,
    P: TaskThread,
    Q: QuarantineTasks,
{
    /// The service, telling Runs' owners of their waiting Gates through
    /// `notifier`, and reading their comments on the Runs' tasks through
    /// `notes`, for the Runs whose owner section is on. A Run whose
    /// definition is quiet tells no one.
    pub fn with_owner_gates<N2: GateNotifier, T2: TaskNotes>(
        self,
        notifier: N2,
        notes: T2,
    ) -> StepServiceImpl<R, E, N2, T2, P, Q> {
        StepServiceImpl {
            repo: self.repo,
            executor: self.executor,
            notifier,
            notes,
            thread: self.thread,
            quarantine_tasks: self.quarantine_tasks,
            usage: self.usage,
        }
    }

    /// The service, posting Runs' updates to their tasks through `thread`,
    /// for the Runs whose task thread section is on.
    pub fn with_task_thread<P2: TaskThread>(
        self,
        thread: P2,
    ) -> StepServiceImpl<R, E, N, T, P2, Q> {
        StepServiceImpl {
            repo: self.repo,
            executor: self.executor,
            notifier: self.notifier,
            notes: self.notes,
            thread,
            quarantine_tasks: self.quarantine_tasks,
            usage: self.usage,
        }
    }

    /// The service, filing through `tasks` a task for each CI job a Run's
    /// `ci` Step quarantined, as the Run's owner, once the Run ends.
    /// Without it, quarantines are kept and listed, but no task is filed.
    pub fn with_quarantine_tasks<Q2: QuarantineTasks>(
        self,
        tasks: Q2,
    ) -> StepServiceImpl<R, E, N, T, P, Q2> {
        StepServiceImpl {
            repo: self.repo,
            executor: self.executor,
            notifier: self.notifier,
            notes: self.notes,
            thread: self.thread,
            quarantine_tasks: tasks,
            usage: self.usage,
        }
    }

    /// The service, checking Runs' token budgets against what `usage`
    /// records their agent sessions used. Without it, a Run with a budget
    /// reads as unread at each check, and ends once it has been unread for
    /// too many in a row.
    pub fn with_usage(mut self, usage: impl RunTokens + 'static) -> Self {
        self.usage = Some(UsageLog(Arc::new(usage)));
        self
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

    /// Whether the Gate of `run`'s Step `seq` is one the Run's own Flow
    /// keeps for a person (`person_only`): no Standing Rule answers it. A
    /// self-improvement Run's Gates, and a Gate whose Step or Flow does not
    /// read, are taken as kept for a person: a lost Flow never lets a rule
    /// answer.
    async fn kept_for_a_person(
        &self,
        run: &Run,
        steps: &[Step],
        seq: u16,
    ) -> Result<bool, StepError> {
        if run.flow.as_deref() == Some(IMPROVE_FLOW) {
            return Ok(true);
        }
        let Some(entry) = steps
            .iter()
            .find(|step| step.seq == seq)
            .map(|step| &step.entry)
        else {
            return Ok(true);
        };
        let Some(kept) = self.repo.run_flow(run.id).await.map_err(internal)? else {
            return Ok(true);
        };
        let Some(flow) = walked_flow(run.id, kept) else {
            return Ok(true);
        };
        Ok(flow
            .entries
            .iter()
            .find(|asking| asking.id == *entry)
            .and_then(|asking| asking.gate.as_ref())
            .is_some_and(|gate| gate.person_only))
    }

    async fn run_of(&self, request: &StepRequest) -> Result<Run, StepError> {
        self.repo
            .get_run(request.key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)
    }
}

impl<R, E, N, T, P, Q> StepService for StepServiceImpl<R, E, N, T, P, Q>
where
    R: RunRepository
        + ReviewRoundRepository
        + MainHoldRepository
        + EvaluationRepository
        + StandingActRepository,
    E: StepExecutor,
    N: GateNotifier,
    T: TaskNotes,
    P: TaskThread,
    Q: QuarantineTasks,
{
    #[tracing::instrument(err, skip_all, fields(run_id = %request.key.run_id, seq = request.key.seq))]
    async fn start_step(&self, request: StepRequest) -> Result<StepStart, StepError> {
        // A Run cancelled before its workflow started takes no Step.
        let run = self.run_of(&request).await?;
        if let Some(end) = run.end {
            return Ok(StepStart::RunEnded(end.outcome));
        }
        // A retry's Steps say which branch they work on, so a worker that
        // cannot read that never takes one; a request that names another
        // branch than the Run's is not the Run's workflow's.
        if request.branch_of != run.retry.map(|retry| retry.branch_of) {
            return Err(StepError::Final(rootcause::report!(
                "the step's branch is not its run's"
            )));
        }
        self.repo
            .start_step(request.key, &request.entry, request.visit)
            .await
            .map_err(internal)?;
        // The Run's first Step starting is the Run starting: for a retry,
        // the first past the Steps it kept.
        let first_seq = run.retry.map_or(1, |retry| retry.first_seq);
        if request.key.seq == first_seq {
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
        // What the Step's entry routes, in the Flow the Run kept as it
        // started, the one its workflow walks. A Run kept before Runs kept
        // their Flow walks an older Flow than the built-in one of its name,
        // and one whose Flow does not read is unknown: neither routes an
        // outcome a pull request Step must ask about (`behind`, `queued`),
        // so they go on as before. An outcome its Flow does not route goes
        // to the entry below, which above `merge` would merge unasked.
        // How many visits the entry has comes from the Flow the Run walks,
        // the built-in one of its name for a Run that kept none.
        let kept = self
            .repo
            .run_flow(request.key.run_id)
            .await
            .map_err(internal)?;
        let kept_definition = kept.as_ref().is_some_and(|kept| kept.definition.is_some());
        let entry = kept
            .and_then(|kept| walked_flow(request.key.run_id, kept))
            .and_then(|flow| {
                flow.entries
                    .into_iter()
                    .find(|entry| entry.id == request.entry)
            });
        let max_visits = visits_bound(entry.as_ref(), &request.step);
        let routes: Vec<Outcome> = entry
            .filter(|_| kept_definition)
            .map(|entry| entry.on.into_keys().collect())
            .unwrap_or_default();
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
            routes: &routes,
            max_visits,
            retry: run.retry,
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
        // The agent and Standing Role that answered are kept beside the
        // answer; a retry keeps the first.
        if let Some(standing) = &answer.standing {
            self.repo
                .record_standing_act(
                    answered.key.run_id,
                    standing.act,
                    Some(answered.key.seq),
                    &standing.by,
                )
                .await
                .map_err(internal)?;
        }

        // A Gate its owner was told of is answered: its notification is
        // done, whoever answered. One that never went out is left alone,
        // as closing it would leave a withdrawal behind.
        let run = self
            .repo
            .get_run(answered.key.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        let gate = self
            .repo
            .list_steps(answered.key.run_id)
            .await
            .map_err(internal)?
            .into_iter()
            .find(|step| step.seq == answered.key.seq)
            .and_then(|step| step.gate);
        if let Some(gate) = &gate
            && notice_may_be_out(&run, gate)
        {
            let id = notice_id(answered.key.run_id, answered.key.seq);
            self.notifier.answered(&run.user_id, id).await;
        }
        // An answer in the stand-in's window closes it, and so the notice
        // that the stand-in answered.
        if let Some(gate) = &gate
            && stand_in_notice_may_be_out(gate)
        {
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
        // A Gate the Run's own Flow keeps for a person (an improve Run's
        // plan and merge) takes no Rule Answer, whatever the Run changes.
        if self.kept_for_a_person(&run, &steps, key.seq).await? {
            return Ok(None);
        }
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
        let dismissed = match ruling.kind {
            RuleGateKind::Merge => any_dismissed(
                &self
                    .repo
                    .review_rounds(key.run_id)
                    .await
                    .map_err(internal)?,
            ),
            _ => false,
        };
        let plan_blockers_left = ruling.kind == RuleGateKind::Plan
            && steps
                .iter()
                .find(|step| step.seq == key.seq)
                .and_then(|step| step.end.as_ref())
                .is_some_and(|end| blockers_left(&end.output));
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
            plan_blockers_left,
            dismissed,
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
        // of it by is done, as an answer would make it. Only notices that
        // may have gone out are closed, as closing one never sent leaves a
        // withdrawal behind; a retry closes the same ones again.
        let run = self
            .repo
            .get_run(finish.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        let steps = self
            .repo
            .list_steps(finish.run_id)
            .await
            .map_err(internal)?;
        for step in &steps {
            let open = step
                .gate
                .as_ref()
                .is_some_and(|gate| gate.answer.is_none() && notice_may_be_out(&run, gate));
            if open {
                let id = notice_id(finish.run_id, step.seq);
                self.notifier.answered(&run.user_id, id).await;
            }
        }

        // A Run that ends in a stand-in's window closes it: the notice
        // that the stand-in answered is done, as the window closing would
        // make it.
        for step in &steps {
            let in_window = step
                .gate
                .as_ref()
                .is_some_and(|gate| gate.answer.is_none() && stand_in_notice_may_be_out(gate));
            if in_window {
                let id = stand_in_id(finish.run_id, step.seq);
                self.notifier.answered(&run.user_id, id).await;
            }
        }

        // A notify-only Gate's notice is news of a Run under way: once the
        // Run ends it is done. No record says which entries told, so each
        // visited entry whose Gate is notify-only in the Run's Flow is
        // closed, whatever the owner section says, as the notice was sent.
        let flow = self
            .repo
            .run_flow(finish.run_id)
            .await
            .map_err(internal)?
            .and_then(|kept| walked_flow(finish.run_id, kept));
        let visited: BTreeSet<_> = steps.iter().map(|step| &step.entry).collect();
        for entry in told_entries(flow.as_ref(), visited) {
            let id = told_id(finish.run_id, entry);
            self.notifier.answered(&run.user_id, id).await;
        }
        // A Run its token budget ended tells its owner, once it reads as
        // ended: the same notice on a retry.
        if let Some(notice) = budget_notice(&run) {
            self.notifier.notify(notice).await;
        }
        quarantine::file_tasks(&self.repo, &self.quarantine_tasks, &run).await;
        self.post(finish.run_id, RunUpdate::Ended).await;
        Ok(())
    }

    #[tracing::instrument(err, skip_all, fields(run_id = %check.run_id, unread = check.unread))]
    async fn check_budget(&self, check: BudgetCheck) -> Result<BudgetVerdict, StepError> {
        let run = self
            .repo
            .get_run(check.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;
        // A retry after the stop was kept finds it kept, whatever the log
        // reads now.
        if run.budget_stop.is_some() {
            return Ok(BudgetVerdict::Stopped);
        }
        let Some(budget) = budget_of(&run) else {
            return Ok(BudgetVerdict::Within);
        };
        let used = match &self.usage {
            Some(usage) => {
                let sessions = self.repo.step_sessions(run.id).await.map_err(internal)?;
                usage.0.used(&run.user_id, &sessions).await
            }
            None => Err(rootcause::report!("no usage log to read")),
        };
        let used = match used {
            Ok(Some(used)) => Some(used),
            // Sessions that recorded no usage at all ran on an agent whose
            // runtime is not measured: the budget cannot apply to them.
            Ok(None) => {
                tracing::info!(
                    "a run's agent sessions recorded no token use: its budget is not measured"
                );
                Some(0)
            }
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "a run's token use was not read: its step goes on unchecked"
                );
                None
            }
        };
        match verdict(budget, used, check.unread) {
            Ok(verdict) => Ok(verdict),
            Err(stop) => {
                self.repo
                    .record_budget_stop(run.id, stop)
                    .await
                    .map_err(internal)?;
                Ok(BudgetVerdict::Stopped)
            }
        }
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
