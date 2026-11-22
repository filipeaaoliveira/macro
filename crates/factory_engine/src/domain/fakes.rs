//! In-memory stand-ins for the factory's records, for the domain's tests.

mod automations;
mod board;
mod evaluation;
mod flaws;
mod gate_owner;
mod gate_replies;
mod main_hold;
mod merge_queue;
pub(crate) mod self_improvement;
mod standing_acts;
mod steering;
mod task_thread;
mod token_budget;

pub(crate) use evaluation::{FakeEvaluation, FakeJudge};
pub(crate) use gate_owner::{FakeNotifier, FakeTaskNotes};
pub(crate) use gate_replies::FakeGateModel;
pub(crate) use main_hold::{FakeHoldTasks, FakeMain, FakeWatches};
pub(crate) use task_thread::FakeTaskThread;
pub(crate) use token_budget::FakeRunTokens;

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use agent_session::domain::session_task::TaskDocumentId;
use chrono::Utc;
use factory_records::domain::automations::{Automation, FiringEnd};
use factory_records::domain::models::{
    BudgetStop, CheckResult, CheckRound, CheckRoundEnd, CheckRoundOutcome, CheckRoundStart,
    ClaimedSession, ControlFileChanges, EntryId, Gate, GateAnswer, GateKind, HandBack, HandedBack,
    NewRun, NewSteer, Outcome, PassedOver, QueueHold, QueuePlaces, Role, Run, RunDefinition,
    RunEnd, RunFlow, RunId, RunInsert, RunOutcome, SessionStep, SteerRefusal, Steered, Step,
    StepEnd, StepKey, StepSession, StepSteering, StoppingSession, TaskLink, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, ProjectRules, ProjectRulesSet, StandingRule, StandingRulesRepository,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::check_context::ComparedChanges;
use crate::domain::models::{
    BranchDeletion, BranchUpdating, Check, CheckState, CheckSuite, CodeRepository, CommitMade,
    FinishedJob, GITHUB_COMMITTER, HeadChecks, Merging, OpenPullRequest, Opening, PullRequest,
    PullRequestState, RepositoryAccess, RequiredChecks, run_branch,
};
use crate::domain::ports::{
    FileRefusal, FollowUpTask, PullRequests, Retryable, TaskAccess, TaskDirectory, UserDirectory,
};
use crate::domain::review_rounds::ChangedFile;

mod review_rounds;

/// The session an agent Step claimed, and what it handed back.
#[derive(Clone)]
pub(crate) struct FakeSession {
    pub(crate) key: StepKey,
    pub(crate) session: StepSession,
    pub(crate) offered: Vec<Outcome>,
    pub(crate) handed: Option<HandBack>,
    /// The later prompt the agent works on, once there is one, and its
    /// place among the Step's later prompts.
    pub(crate) turn: Option<Uuid>,
    pub(crate) turn_place: Option<u16>,
    pub(crate) released: bool,
    /// When an attempt last asked for the session to be opened.
    pub(crate) prepared_at: Option<chrono::DateTime<Utc>>,
    pub(crate) opened: bool,
    /// The session whose setup commands passed, and how many of them.
    pub(crate) setup: Option<(Uuid, u16)>,
    /// The sessions the Step left for a new one, which its Run's token
    /// budget still counts.
    pub(crate) left: Vec<Uuid>,
}

#[derive(Debug, thiserror::Error)]
#[error("fake failure")]
pub(crate) struct FakeError;

/// What [`FakeGithub`] fails with: GitHub down, worth trying again, or a
/// refusal of the owner, which is not.
#[derive(Debug, thiserror::Error)]
pub(crate) enum FakeGithubError {
    #[error("github is down")]
    Down,
    #[error("github refused the owner")]
    Refused,
    #[error("the file is over 1 MB")]
    TooLarge,
}

impl Retryable for FakeGithubError {
    fn retryable(&self) -> bool {
        matches!(self, Self::Down)
    }
}

/// A fake's failure is the transport's: worth trying again.
impl Retryable for FakeError {
    fn retryable(&self) -> bool {
        true
    }
}

/// An answer recorded for a Gate, with the Control Files it approved.
pub(crate) type Approval = (StepKey, Outcome, Option<Vec<String>>);

/// A follow-up task [`FakeRepo`] made: whose, the task it follows up, and
/// what it says.
#[derive(Debug, Clone)]
pub(crate) struct FollowUpMade {
    pub(crate) owner: String,
    pub(crate) of: Option<Uuid>,
    pub(crate) task: FollowUpTask,
    pub(crate) id: Uuid,
}

/// A comment on a follow-up task, by its author.
#[derive(Debug, Clone)]
pub(crate) struct FollowUpComment {
    pub(crate) owner: String,
    pub(crate) task_id: Uuid,
    pub(crate) text: String,
}

/// Runs, Steps and sessions in memory, with the same first-write-wins rules
/// as the Postgres repository.
#[derive(Clone, Default)]
pub(crate) struct FakeRepo {
    pub(crate) runs: Arc<Mutex<BTreeMap<RunId, Run>>>,
    /// The merge queues and the updates of Runs' branches.
    pub(crate) queue: Arc<Mutex<merge_queue::FakeQueue>>,
    /// The built-in Flow each Run walks.
    pub(crate) flows: Arc<Mutex<BTreeMap<RunId, String>>>,
    pub(crate) steps: Arc<Mutex<Vec<(RunId, Step)>>>,
    /// Each agent Step's session, what it offers, and its latest hand back.
    pub(crate) sessions: Arc<Mutex<Vec<FakeSession>>>,
    /// Each Step's steering by its owner.
    pub(crate) steering: Arc<Mutex<HashMap<StepKey, steering::FakeSteering>>>,
    /// The CI waits kept, in order, the last of each Step.
    pub(crate) ci_waits: Arc<Mutex<Vec<factory_records::domain::quality::CiWait>>>,
    /// Each Step's rounds of checks, in order.
    pub(crate) check_rounds: Arc<Mutex<Vec<(StepKey, CheckRound)>>>,
    /// The task documents that exist.
    pub(crate) tasks: Arc<Mutex<BTreeSet<Uuid>>>,
    /// The documents that exist and are not tasks.
    pub(crate) documents: Arc<Mutex<BTreeSet<Uuid>>>,
    /// The task documents a user cannot see: their id and the task's.
    pub(crate) hidden: Arc<Mutex<BTreeSet<(String, Uuid)>>>,
    /// How many batch view checks were asked.
    pub(crate) view_checks: Arc<Mutex<usize>>,
    /// The commit each Run last asked a person to merge.
    pub(crate) merge_heads: Arc<Mutex<BTreeMap<RunId, String>>>,
    /// Told each time a Gate opens.
    pub(crate) gate_opened: Arc<tokio::sync::Notify>,
    /// The Control Files each Run's branch last changed, also for Runs the
    /// fake does not keep.
    pub(crate) control_file_changes: Arc<Mutex<BTreeMap<RunId, ControlFileChanges>>>,
    /// The answers recorded, by Gate, with the Control Files they approved.
    pub(crate) approvals: Arc<Mutex<Vec<Approval>>>,
    /// The Rule Answers recorded, by Gate, with the rule that gave each.
    pub(crate) rule_answers: Arc<Mutex<Vec<(StepKey, Outcome, StandingRule)>>>,
    /// The agents' accounts, by their id.
    pub(crate) agent_accounts: Arc<Mutex<BTreeSet<String>>>,
    /// The names people show others, by user id.
    pub(crate) display_names: Arc<Mutex<BTreeMap<String, String>>>,
    /// The team each person is on, by their id.
    pub(crate) teams: Arc<Mutex<BTreeMap<String, TeamMembership>>>,
    /// The Control File patterns each team set.
    pub(crate) control_files: Arc<Mutex<BTreeMap<Uuid, Vec<String>>>>,
    /// The Factory Definition each team names, by its repository's URL.
    pub(crate) team_definitions: Arc<Mutex<BTreeMap<Uuid, String>>>,
    /// Who named each team's Definition, by team.
    pub(crate) definition_namers: Arc<Mutex<BTreeMap<Uuid, String>>>,
    /// Failures and self-improvement Runs.
    pub(crate) improvement: Arc<Mutex<self_improvement::FakeImprovement>>,
    /// How many times one Run's Flow was read on its own.
    pub(crate) flow_reads: Arc<Mutex<usize>>,
    /// The Runs each read of several Runs' Flows asked for, in order.
    pub(crate) runs_flows_reads: Arc<Mutex<Vec<Vec<RunId>>>>,
    /// The Flow each Run kept, and whether it asks, by Run.
    pub(crate) kept_flows: Arc<Mutex<BTreeMap<RunId, (serde_json::Value, bool)>>>,
    /// The Agents of the Definition each Run started under, as kept.
    pub(crate) definition_agents: Arc<Mutex<BTreeMap<RunId, serde_json::Value>>>,
    /// The Scorers, Experiments and Benchmarks of the Definition each Run
    /// started under, as kept.
    pub(crate) definition_evaluations: Arc<Mutex<BTreeMap<RunId, serde_json::Value>>>,
    /// The bot of each agent session, with the session's owner, by the
    /// session's id.
    pub(crate) session_bots: Arc<Mutex<BTreeMap<Uuid, (String, Uuid)>>>,
    /// Whether each Run's last cancel came from its owner in person.
    pub(crate) cancelled_in_person: Arc<Mutex<BTreeMap<RunId, bool>>>,
    /// The standing acts kept, in order.
    pub(crate) standing_acts: Arc<
        Mutex<
            Vec<(
                RunId,
                factory_records::domain::standing_acts::StandingActRecord,
            )>,
        >,
    >,
    /// The Standing Rules each team set.
    pub(crate) team_rules: Arc<Mutex<BTreeMap<Uuid, OrchestratorDefinition>>>,
    /// The Standing Rules each team set for a Project, by team and Project.
    pub(crate) project_rules: Arc<Mutex<BTreeMap<(Uuid, Uuid), OrchestratorDefinition>>>,
    /// The Projects that exist.
    pub(crate) projects: Arc<Mutex<BTreeSet<Uuid>>>,
    /// The Project each task is in.
    pub(crate) task_projects: Arc<Mutex<BTreeMap<Uuid, Uuid>>>,
    /// Whether reading a task's Project fails, as an outage would.
    pub(crate) project_reads_fail: Arc<Mutex<bool>>,
    /// The Projects a person may not edit: their id and the Project's.
    pub(crate) uneditable_projects: Arc<Mutex<BTreeSet<(String, Uuid)>>>,
    /// The merges Runs made and the holds on their bases.
    pub(crate) main: Arc<Mutex<FakeMain>>,
    /// Each Run's review rounds, with what their reviewers found.
    pub(crate) review_rounds:
        Arc<Mutex<Vec<(RunId, factory_records::domain::review_rounds::ReviewRound)>>>,
    /// The follow-up tasks made, by their owner and the task each follows
    /// up, in order.
    pub(crate) follow_ups: Arc<Mutex<Vec<FollowUpMade>>>,
    /// How many of the next follow-up tasks fail as an outage would, made
    /// or not as `follow_ups_made_anyway` says.
    pub(crate) follow_ups_fail: Arc<Mutex<usize>>,
    /// Whether a follow-up task that fails is made all the same, as when
    /// its answer is lost.
    pub(crate) follow_ups_made_anyway: Arc<Mutex<bool>>,
    /// Whether document storage refuses follow-up tasks.
    pub(crate) follow_ups_refused: Arc<Mutex<bool>>,
    /// The comments made on follow-up tasks, in order.
    pub(crate) follow_up_comments: Arc<Mutex<Vec<FollowUpComment>>>,
    /// The calls that fail as an outage would, by name: how many calls of
    /// it pass first, then how many fail.
    pub(crate) failing: Arc<Mutex<BTreeMap<&'static str, (usize, usize)>>>,
    /// The prompt each reviewer's Step was asked again in.
    pub(crate) reasks: Arc<Mutex<Vec<(StepKey, Uuid)>>>,
    /// The Automations kept, by id, with their webhook's secret.
    pub(crate) automations: Arc<Mutex<BTreeMap<Uuid, KeptAutomation>>>,
    /// Each Automation's firings, by its id and the firing's key, with what
    /// they came to.
    pub(crate) firings: Arc<Mutex<BTreeMap<FiringKey, Option<FiringEnd>>>>,
    /// Experiment arms, Scorers' judgments and Benchmark suites.
    pub(crate) evaluation: Arc<Mutex<FakeEvaluation>>,
}

/// A firing: its Automation and its key.
pub(crate) type FiringKey = (Uuid, String);

/// An Automation kept, with its webhook's secret.
pub(crate) type KeptAutomation = (Automation, Option<String>);

/// Everyone has an account but `macro|nobody@example.com`.
impl UserDirectory for FakeRepo {
    type Err = FakeError;

    async fn user_exists(&self, user_id: &MacroUserIdStr<'static>) -> Result<bool, FakeError> {
        Ok(user_id.as_ref() != "macro|nobody@example.com")
    }
}

/// Every task is visible to everyone it is not hidden from.
impl TaskDirectory for FakeRepo {
    type Err = FakeError;

    async fn task_access(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task: &TaskDocumentId,
    ) -> Result<TaskAccess, FakeError> {
        let id = Uuid::parse_str(task.as_str()).map_err(|_| FakeError)?;
        let hidden = (user_id.as_ref().to_string(), id);
        if self.hidden.lock().expect("hidden").contains(&hidden) {
            Ok(TaskAccess::NotFound)
        } else if self.tasks.lock().expect("tasks").contains(&id) {
            Ok(TaskAccess::Visible)
        } else if self.documents.lock().expect("documents").contains(&id) {
            Ok(TaskAccess::NotATask)
        } else {
            Ok(TaskAccess::NotFound)
        }
    }

    async fn viewable_documents(
        &self,
        user_id: &MacroUserIdStr<'static>,
        document_ids: &[Uuid],
    ) -> Result<BTreeSet<Uuid>, FakeError> {
        *self.view_checks.lock().expect("view checks") += 1;
        // As entity access: whether the document is a task, or trashed, is
        // not its to say.
        let hidden = self.hidden.lock().expect("hidden");
        Ok(document_ids
            .iter()
            .copied()
            .filter(|id| !hidden.contains(&(user_id.as_ref().to_string(), *id)))
            .collect())
    }

    async fn task_project(
        &self,
        _user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Result<Option<Uuid>, FakeError> {
        if *self.project_reads_fail.lock().expect("fail") {
            return Err(FakeError);
        }
        Ok(self
            .task_projects
            .lock()
            .expect("task projects")
            .get(&task_id)
            .copied())
    }

    async fn may_edit_project(
        &self,
        user_id: &MacroUserIdStr<'static>,
        project_id: Uuid,
    ) -> Result<bool, FakeError> {
        let uneditable = (user_id.as_ref().to_string(), project_id);
        Ok(!self
            .uneditable_projects
            .lock()
            .expect("uneditable")
            .contains(&uneditable))
    }

    async fn create_follow_up(
        &self,
        user_id: &MacroUserIdStr<'static>,
        of: Option<Uuid>,
        follow_up: &FollowUpTask,
        task_id: Uuid,
    ) -> Result<crate::domain::ports::FollowUpMade, FakeError> {
        use crate::domain::ports::FollowUpMade as Made;
        if self.tasks.lock().expect("tasks").contains(&task_id) {
            return Ok(Made::Made);
        }
        if *self.follow_ups_refused.lock().expect("refused") {
            return Ok(Made::Refused);
        }
        let fails = {
            let mut fails = self.follow_ups_fail.lock().expect("fail");
            let failing = *fails > 0;
            *fails = fails.saturating_sub(1);
            failing
        } || self.fails("create_follow_up");
        if !fails || *self.follow_ups_made_anyway.lock().expect("anyway") {
            self.tasks.lock().expect("tasks").insert(task_id);
            self.follow_ups
                .lock()
                .expect("follow ups")
                .push(FollowUpMade {
                    owner: user_id.as_ref().to_string(),
                    of,
                    task: follow_up.clone(),
                    id: task_id,
                });
        }
        if fails {
            return Err(FakeError);
        }
        Ok(Made::Made)
    }

    /// A comment quotes a passage the follow-up task says once, as
    /// document storage anchors it.
    async fn comment_on_follow_up(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
        quote: &str,
        text: &str,
    ) -> Result<(), FakeError> {
        if self.fails("comment_on_follow_up") {
            return Err(FakeError);
        }
        let quoted_once = self
            .follow_ups
            .lock()
            .expect("follow ups")
            .iter()
            .any(|made| made.id == task_id && made.task.markdown.matches(quote).count() == 1);
        if !quoted_once {
            return Err(FakeError);
        }
        self.follow_up_comments
            .lock()
            .expect("comments")
            .push(FollowUpComment {
                owner: user_id.as_ref().to_string(),
                task_id,
                text: text.to_string(),
            });
        Ok(())
    }
}

impl FakeRepo {
    /// The Runs that reached a Gate, answered or not.
    fn gated_runs(&self) -> BTreeSet<RunId> {
        self.steps
            .lock()
            .expect("steps")
            .iter()
            .filter(|(_, step)| step.gate.is_some())
            .map(|(run, _)| *run)
            .collect()
    }

    /// As the database: the places `user_id` holds, counted up to `limit`
    /// in all, Runs waiting at a Gate the stand-in has not answered first.
    fn queue_places(
        &self,
        user_id: &MacroUserIdStr<'static>,
        asking_flows: &[&str],
        limit: u16,
    ) -> QueuePlaces {
        let gated = self.gated_runs();
        let waiting: BTreeSet<RunId> = self
            .steps
            .lock()
            .expect("steps")
            .iter()
            .filter(|(_, step)| {
                step.gate
                    .as_ref()
                    .is_some_and(|gate| gate.answer.is_none() && gate.stand_in.is_none())
            })
            .map(|(run, _)| *run)
            .collect();
        let asks = self.asks(asking_flows);
        let mut places = QueuePlaces::default();
        for run in self.runs.lock().expect("runs").values() {
            if run.user_id != *user_id || run.end.is_some() || places.held() >= limit {
                continue;
            }
            if waiting.contains(&run.id) {
                places.waiting += 1;
            } else if !gated.contains(&run.id) && asks.contains(&run.id) {
                places.starting += 1;
            }
        }
        places
    }

    /// As the database: the Runs whose Flow asks before their first Gate,
    /// as each kept it, else by `asking_flows`, its Flow's name.
    fn asks(&self, asking_flows: &[&str]) -> BTreeSet<RunId> {
        let kept = self.kept_flows.lock().expect("kept flows").clone();
        self.flows
            .lock()
            .expect("flows")
            .iter()
            .filter(|(run, flow)| match kept.get(run) {
                Some((_, asks)) => *asks,
                None => asking_flows.contains(&flow.as_str()),
            })
            .map(|(run, _)| *run)
            .collect()
    }

    /// Fail the call `name` once, after `passing` calls of it pass.
    pub(crate) fn fail_once_after(&self, name: &'static str, passing: usize) {
        self.failing
            .lock()
            .expect("failing")
            .insert(name, (passing, 1));
    }

    /// Whether this call of `name` fails, as `fail_once_after` set.
    pub(crate) fn fails(&self, name: &'static str) -> bool {
        let mut failing = self.failing.lock().expect("failing");
        match failing.get_mut(name) {
            Some((passing, _)) if *passing > 0 => {
                *passing -= 1;
                false
            }
            Some((_, failures)) if *failures > 0 => {
                *failures -= 1;
                true
            }
            _ => false,
        }
    }

    fn with_step<T>(&self, key: StepKey, change: impl FnOnce(&mut Step) -> T) -> Option<T> {
        let mut steps = self.steps.lock().expect("steps");
        steps
            .iter_mut()
            .find(|(run, step)| *run == key.run_id && step.seq == key.seq)
            .map(|(_, step)| change(step))
    }

    fn rounds_of(&self, key: StepKey) -> Vec<CheckRound> {
        self.check_rounds
            .lock()
            .expect("rounds")
            .iter()
            .filter(|(step, _)| *step == key)
            .map(|(_, round)| round.clone())
            .collect()
    }

    fn step_ended(&self, key: StepKey) -> bool {
        self.with_step(key, |step| step.end.is_some())
            .unwrap_or(false)
    }
}

impl RunRepository for FakeRepo {
    type Err = FakeError;

    async fn insert_run(&self, new: NewRun<'_>) -> Result<RunInsert, FakeError> {
        let NewRun {
            id,
            flow,
            request,
            user_id,
            task_id,
            project_id,
            repository,
            created_in_person,
            control_file_patterns,
            control_files_team_id,
            standing_rules,
            automation,
            definition,
            kept_flow,
        } = new;
        if let Some(task_id) = task_id
            && !self.tasks.lock().expect("tasks").contains(&task_id)
        {
            return Ok(RunInsert::NoSuchTask);
        }
        // As the database's index: an Automation starts no Run on a task
        // while a Run an Automation started runs there.
        if automation.is_some()
            && task_id.is_some()
            && self
                .runs
                .lock()
                .expect("runs")
                .values()
                .any(|run| run.task_id == task_id && run.automation.is_some() && run.end.is_none())
        {
            return Ok(RunInsert::TaskBusy);
        }
        // As the database: its Queue limit is not kept.
        let mut standing_rules = standing_rules;
        standing_rules.definition.queue_limit = None;
        let run = Run {
            id,
            request: request.to_string(),
            user_id: user_id.clone(),
            flow: Some(flow.to_string()),
            created_at: Utc::now(),
            task_id,
            project_id,
            repository: repository.cloned(),
            created_in_person,
            control_file_patterns: Some(control_file_patterns.to_vec()),
            control_files_team_id,
            control_files: None,
            pull_request_url: None,
            standing_rules: Some(standing_rules),
            automation: automation.map(|start| start.automation.clone()),
            definition: definition.map(|definition| RunDefinition {
                repo_url: definition.repo_url.to_string(),
                commit: definition.commit.to_string(),
            }),
            end: None,
            budget_stop: None,
        };
        if let Some(kept) = kept_flow {
            self.kept_flows
                .lock()
                .expect("kept flows")
                .insert(id, (kept.definition.clone(), kept.asks));
        }
        if let Some(definition) = definition {
            self.definition_agents
                .lock()
                .expect("definition agents")
                .insert(id, definition.agents.clone());
            self.definition_evaluations
                .lock()
                .expect("definition evaluations")
                .insert(id, definition.evaluation.clone());
        }
        self.runs.lock().expect("runs").insert(id, run.clone());
        self.flows
            .lock()
            .expect("flows")
            .insert(id, flow.to_string());
        Ok(RunInsert::Inserted(Box::new(run)))
    }

    async fn link_task(&self, id: RunId, task_id: Uuid) -> Result<TaskLink, FakeError> {
        if !self.tasks.lock().expect("tasks").contains(&task_id) {
            return Ok(TaskLink::NoSuchTask);
        }
        match self.runs.lock().expect("runs").get_mut(&id) {
            Some(run) => {
                run.task_id = Some(task_id);
                Ok(TaskLink::Linked)
            }
            None => Ok(TaskLink::NoSuchRun),
        }
    }

    async fn set_merge_head(&self, id: RunId, sha: &str) -> Result<(), FakeError> {
        self.merge_heads
            .lock()
            .expect("merge heads")
            .insert(id, sha.to_string());
        Ok(())
    }

    async fn record_ci_wait(
        &self,
        wait: &factory_records::domain::quality::CiWait,
    ) -> Result<(), FakeError> {
        if self.fails("record_ci_wait") {
            return Err(FakeError);
        }
        let mut waits = self.ci_waits.lock().expect("ci waits");
        match waits.iter_mut().find(|kept| kept.key == wait.key) {
            Some(kept) if (&kept.head_sha, kept.verdict) == (&wait.head_sha, wait.verdict) => {}
            Some(kept) => *kept = wait.clone(),
            None => waits.push(wait.clone()),
        }
        Ok(())
    }

    async fn last_ci_wait(
        &self,
        run_id: RunId,
        before: u16,
    ) -> Result<Option<factory_records::domain::quality::LastCiWait>, FakeError> {
        if self.fails("last_ci_wait") {
            return Err(FakeError);
        }
        Ok(self
            .ci_waits
            .lock()
            .expect("ci waits")
            .iter()
            .filter(|kept| kept.key.run_id == run_id && kept.key.seq < before)
            .max_by_key(|kept| kept.key.seq)
            .map(|kept| factory_records::domain::quality::LastCiWait {
                head_sha: kept.head_sha.clone(),
                jobs: kept
                    .jobs
                    .iter()
                    .filter(|job| !job.replaced)
                    .map(|job| job.name.clone())
                    .collect(),
            }))
    }

    async fn set_pull_request_url(&self, id: RunId, url: &str) -> Result<(), FakeError> {
        if self.fails("set_pull_request_url") {
            return Err(FakeError);
        }
        if let Some(run) = self.runs.lock().expect("runs").get_mut(&id) {
            run.pull_request_url = Some(url.to_string());
        }
        Ok(())
    }

    async fn merge_head(&self, id: RunId) -> Result<Option<String>, FakeError> {
        Ok(self
            .merge_heads
            .lock()
            .expect("merge heads")
            .get(&id)
            .cloned())
    }

    async fn delete_run(&self, id: RunId) -> Result<(), FakeError> {
        self.runs.lock().expect("runs").remove(&id);
        Ok(())
    }

    async fn get_run(&self, id: RunId) -> Result<Option<Run>, FakeError> {
        let control_files = self
            .control_file_changes
            .lock()
            .expect("control files")
            .get(&id)
            .cloned();
        Ok(self
            .runs
            .lock()
            .expect("runs")
            .get(&id)
            .cloned()
            .map(|run| Run {
                control_files,
                ..run
            }))
    }

    async fn set_control_file_changes(
        &self,
        id: RunId,
        changes: Option<&ControlFileChanges>,
    ) -> Result<(), FakeError> {
        let mut kept = self.control_file_changes.lock().expect("control files");
        match changes {
            Some(changes) => kept.insert(id, changes.clone()),
            None => kept.remove(&id),
        };
        Ok(())
    }

    async fn display_name(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<Option<String>, FakeError> {
        Ok(self
            .display_names
            .lock()
            .expect("display names")
            .get(user_id.as_ref())
            .cloned())
    }

    async fn is_agent_account(&self, user_id: &MacroUserIdStr<'static>) -> Result<bool, FakeError> {
        Ok(self
            .agent_accounts
            .lock()
            .expect("agent accounts")
            .contains(user_id.as_ref()))
    }

    async fn team_of(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<Option<TeamMembership>, FakeError> {
        let agent = self.is_agent_account(user_id).await?;
        Ok(self
            .teams
            .lock()
            .expect("teams")
            .get(user_id.as_ref())
            .map(|team| TeamMembership {
                admin: team.admin && !agent,
                ..*team
            }))
    }

    async fn team_control_files(&self, team_id: Uuid) -> Result<Option<Vec<String>>, FakeError> {
        Ok(self
            .control_files
            .lock()
            .expect("control files")
            .get(&team_id)
            .cloned())
    }

    async fn team_definition(&self, team_id: Uuid) -> Result<Option<String>, FakeError> {
        Ok(self
            .team_definitions
            .lock()
            .expect("team definitions")
            .get(&team_id)
            .cloned())
    }

    async fn set_team_definition(
        &self,
        team_id: Uuid,
        repo_url: Option<&str>,
        named_by: &MacroUserIdStr<'static>,
    ) -> Result<(), FakeError> {
        {
            let mut kept = self.team_definitions.lock().expect("team definitions");
            let mut namers = self.definition_namers.lock().expect("definition namers");
            match repo_url {
                Some(repo_url) => {
                    kept.insert(team_id, repo_url.to_string());
                    namers.insert(team_id, named_by.as_ref().to_string());
                }
                None => {
                    kept.remove(&team_id);
                    namers.remove(&team_id);
                }
            };
        }
        self.close_other_open(team_id, repo_url);
        Ok(())
    }

    async fn runs_flows(&self, ids: &[RunId]) -> Result<BTreeMap<RunId, RunFlow>, FakeError> {
        self.runs_flows_reads
            .lock()
            .expect("runs flows reads")
            .push(ids.to_vec());
        let names = self.flows.lock().expect("flows").clone();
        let kept = self.kept_flows.lock().expect("kept flows").clone();
        Ok(ids
            .iter()
            .filter_map(|id| {
                let name = names.get(id)?.clone();
                let flow = RunFlow {
                    name: Some(name),
                    definition: kept.get(id).map(|(flow, _)| flow.clone()),
                };
                Some((*id, flow))
            })
            .collect())
    }

    async fn run_flow(&self, id: RunId) -> Result<Option<RunFlow>, FakeError> {
        *self.flow_reads.lock().expect("flow reads") += 1;
        let Some(name) = self.flows.lock().expect("flows").get(&id).cloned() else {
            return Ok(None);
        };
        let definition = self
            .kept_flows
            .lock()
            .expect("kept flows")
            .get(&id)
            .map(|(flow, _)| flow.clone());
        Ok(Some(RunFlow {
            name: Some(name),
            definition,
        }))
    }

    async fn run_definition_agents(
        &self,
        id: RunId,
    ) -> Result<Option<serde_json::Value>, FakeError> {
        Ok(self
            .definition_agents
            .lock()
            .expect("definition agents")
            .get(&id)
            .cloned())
    }

    async fn set_team_control_files(
        &self,
        team_id: Uuid,
        patterns: Option<&[String]>,
    ) -> Result<(), FakeError> {
        let mut kept = self.control_files.lock().expect("control files");
        match patterns {
            Some(patterns) => kept.insert(team_id, patterns.to_vec()),
            None => kept.remove(&team_id),
        };
        Ok(())
    }

    async fn list_steps(&self, id: RunId) -> Result<Vec<Step>, FakeError> {
        let steps = self.steps.lock().expect("steps");
        let mut steps: Vec<Step> = steps
            .iter()
            .filter(|(run, _)| *run == id)
            .map(|(_, step)| step.clone())
            .collect();
        steps.sort_by_key(|step| step.seq);
        for step in &mut steps {
            let key = StepKey {
                run_id: id,
                seq: step.seq,
            };
            step.checks = self.rounds_of(key);
            step.terminal = self.terminal_of(key, step.end.is_some());
        }
        Ok(steps)
    }

    async fn list_task_runs(
        &self,
        task_id: Uuid,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> Result<Vec<Run>, FakeError> {
        let mut runs: Vec<Run> = self
            .runs
            .lock()
            .expect("runs")
            .values()
            .filter(|run| run.task_id == Some(task_id) && run.user_id == *user_id)
            .cloned()
            .collect();
        runs.sort_by_key(|run| std::cmp::Reverse((run.created_at, run.id)));
        runs.truncate(usize::from(limit));
        Ok(runs)
    }

    async fn list_waiting_runs(
        &self,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> Result<Vec<Run>, FakeError> {
        let waiting: BTreeSet<RunId> = self
            .steps
            .lock()
            .expect("steps")
            .iter()
            .filter(|(_, step)| step.gate.as_ref().is_some_and(|gate| gate.answer.is_none()))
            .map(|(run, _)| *run)
            .collect();
        let mut runs: Vec<Run> = self
            .runs
            .lock()
            .expect("runs")
            .values()
            .filter(|run| run.user_id == *user_id && run.end.is_none() && waiting.contains(&run.id))
            .cloned()
            .collect();
        runs.sort_by_key(|run| std::cmp::Reverse((run.created_at, run.id)));
        runs.truncate(usize::from(limit));
        Ok(runs)
    }

    async fn insert_run_in_queue(
        &self,
        run: NewRun<'_>,
        hold: QueueHold<'_>,
    ) -> Result<RunInsert, FakeError> {
        // As the database: the places the owner holds before it, counted up
        // to the limit.
        if let Some(limit) = hold.limit {
            let places = self.queue_places(run.user_id, hold.asking_flows, limit);
            if places.held() >= limit {
                return Ok(RunInsert::QueueFull(places));
            }
        }
        self.insert_run(run).await
    }

    async fn list_starting_runs(
        &self,
        user_id: &MacroUserIdStr<'static>,
        asking_flows: &[&str],
        limit: u16,
    ) -> Result<Vec<Run>, FakeError> {
        let gated = self.gated_runs();
        let asks = self.asks(asking_flows);
        let mut runs: Vec<Run> = self
            .runs
            .lock()
            .expect("runs")
            .values()
            .filter(|run| {
                run.user_id == *user_id
                    && run.end.is_none()
                    && !gated.contains(&run.id)
                    && asks.contains(&run.id)
            })
            .cloned()
            .collect();
        runs.sort_by_key(|run| std::cmp::Reverse((run.created_at, run.id)));
        runs.truncate(usize::from(limit));
        Ok(runs)
    }

    async fn list_runs_steps(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<Step>>, FakeError> {
        let mut steps = BTreeMap::new();
        for id in ids {
            let run_steps = self.list_steps(*id).await?;
            if !run_steps.is_empty() {
                steps.insert(*id, run_steps);
            }
        }
        Ok(steps)
    }

    async fn start_step(&self, key: StepKey, entry: &EntryId, visit: u16) -> Result<(), FakeError> {
        if self.with_step(key, |_| ()).is_some() {
            return Ok(());
        }
        self.steps.lock().expect("steps").push((
            key.run_id,
            Step {
                seq: key.seq,
                entry: entry.clone(),
                visit,
                started_at: Utc::now(),
                end: None,
                gate: None,
                role: None,
                agent: None,
                passed_over: Vec::new(),
                checks: Vec::new(),
                terminal: Default::default(),
            },
        ));
        Ok(())
    }

    async fn end_step(
        &self,
        key: StepKey,
        outcome: &Outcome,
        output: &str,
    ) -> Result<(), FakeError> {
        self.with_step(key, |step| {
            if step.end.is_none() {
                step.end = Some(StepEnd {
                    outcome: outcome.clone(),
                    output: output.to_string(),
                    ended_at: Utc::now(),
                });
            }
        });
        Ok(())
    }

    async fn open_gate(
        &self,
        key: StepKey,
        question: &str,
        answers: &[Outcome],
    ) -> Result<(), FakeError> {
        self.with_step(key, |step| {
            let kind = GateKind::of(step.end.as_ref().map(|end| &end.outcome));
            step.gate.get_or_insert_with(|| Gate {
                asked_back: None,
                stand_in: None,
                stand_in_declined: None,
                kind,
                question: question.to_string(),
                answers: answers.to_vec(),
                opened_at: Utc::now(),
                answer: None,
            });
        });
        self.gate_opened.notify_one();
        Ok(())
    }

    async fn answer_gate(
        &self,
        key: StepKey,
        answer: &Outcome,
        note: Option<&str>,
        user_id: &MacroUserIdStr<'static>,
        approved_control_files: Option<&[String]>,
        agent_session: Option<Uuid>,
    ) -> Result<(), FakeError> {
        self.with_step(key, |step| {
            if let Some(gate) = step.gate.as_mut()
                && gate.answer.is_none()
            {
                gate.answer = Some(GateAnswer {
                    reply_confidence: None,
                    by_stand_in: false,
                    agent_session_id: agent_session,
                    answer: answer.clone(),
                    note: note.map(str::to_string),
                    user_id: Some(user_id.clone()),
                    rule: None,
                    approved_control_files: approved_control_files.map(<[String]>::to_vec),
                    answered_at: Utc::now(),
                });
            }
        });
        self.approvals.lock().expect("approvals").push((
            key,
            answer.clone(),
            approved_control_files.map(<[String]>::to_vec),
        ));
        Ok(())
    }

    async fn answer_gate_by_rule(
        &self,
        key: StepKey,
        answer: &Outcome,
        rule: StandingRule,
    ) -> Result<Option<GateAnswer>, FakeError> {
        let kept = self.with_step(key, |step| {
            let gate = step.gate.as_mut()?;
            if gate.answer.is_none() {
                gate.answer = Some(GateAnswer {
                    reply_confidence: None,
                    by_stand_in: false,
                    agent_session_id: None,
                    answer: answer.clone(),
                    note: None,
                    user_id: None,
                    rule: Some(rule),
                    approved_control_files: None,
                    answered_at: Utc::now(),
                });
            }
            gate.answer.clone()
        });
        self.rule_answers
            .lock()
            .expect("rule answers")
            .push((key, answer.clone(), rule));
        Ok(kept.flatten())
    }

    async fn claim_session(
        &self,
        key: StepKey,
        session: StepSession,
        role: &Role,
        offered: &[Outcome],
    ) -> Result<ClaimedSession, FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions.iter().find(|claimed| claimed.key == key) {
            return Ok(ClaimedSession {
                session: claimed.session.clone(),
                new: false,
            });
        }
        self.with_step(key, |step| {
            step.role = Some(role.clone());
            step.agent = session.agent.clone();
        });
        sessions.push(FakeSession {
            key,
            session: session.clone(),
            offered: offered.to_vec(),
            handed: None,
            turn: None,
            turn_place: None,
            released: false,
            prepared_at: None,
            opened: false,
            setup: None,
            left: Vec::new(),
        });
        Ok(ClaimedSession { session, new: true })
    }

    async fn pass_session(
        &self,
        key: StepKey,
        from: Uuid,
        role: &Role,
        passed: &PassedOver,
        to: Option<&StepSession>,
    ) -> Result<bool, FakeError> {
        if !self.holds_session(key, from).await? {
            return Ok(false);
        }
        let mut sessions = self.sessions.lock().expect("sessions");
        let Some(claimed) = sessions.iter_mut().find(|claimed| {
            claimed.key == key && claimed.handed.is_none() && claimed.turn.is_none()
        }) else {
            return Ok(false);
        };
        if let Some(to) = to {
            claimed.left.push(claimed.session.session_id);
            claimed.session = to.clone();
            claimed.prepared_at = None;
            claimed.opened = false;
            self.open_steering(key);
        }
        self.with_step(key, |step| {
            step.role.get_or_insert_with(|| role.clone());
            if let Some(agent) = to.and_then(|to| to.agent.clone()) {
                step.agent = Some(agent);
            }
            step.agent.get_or_insert_with(|| passed.agent.clone());
            step.passed_over.push(passed.clone());
        });
        Ok(true)
    }

    async fn claimed_session(&self, key: StepKey) -> Result<Option<StepSession>, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        Ok(sessions
            .iter()
            .find(|claimed| claimed.key == key)
            .map(|claimed| claimed.session.clone()))
    }

    async fn release_session(&self, key: StepKey) -> Result<(), FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions.iter_mut().find(|claimed| claimed.key == key) {
            claimed.released = true;
        }
        Ok(())
    }

    async fn record_prepare(
        &self,
        key: StepKey,
        session_id: Uuid,
    ) -> Result<Option<chrono::DateTime<Utc>>, FakeError> {
        let run_ended = self
            .runs
            .lock()
            .expect("runs")
            .get(&key.run_id)
            .is_none_or(|run| run.end.is_some());
        if run_ended || self.step_ended(key) {
            return Ok(None);
        }
        let mut sessions = self.sessions.lock().expect("sessions");
        let claimed = sessions.iter_mut().find(|claimed| {
            claimed.key == key && claimed.session.session_id == session_id && !claimed.released
        });
        Ok(claimed.map(|claimed| {
            let now = Utc::now();
            claimed.prepared_at = Some(now);
            now
        }))
    }

    async fn forget_prepare(
        &self,
        key: StepKey,
        prepared_at: chrono::DateTime<Utc>,
    ) -> Result<(), FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions.iter_mut().find(|claimed| {
            claimed.key == key && !claimed.opened && claimed.prepared_at == Some(prepared_at)
        }) {
            claimed.prepared_at = None;
        }
        Ok(())
    }

    async fn record_open(&self, key: StepKey, session_id: Uuid) -> Result<(), FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions
            .iter_mut()
            .find(|claimed| claimed.key == key && claimed.session.session_id == session_id)
        {
            claimed.opened = true;
        }
        Ok(())
    }

    async fn setup_passed(&self, key: StepKey, session_id: Uuid) -> Result<u16, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        Ok(sessions
            .iter()
            .find(|claimed| claimed.key == key)
            .and_then(|claimed| claimed.setup)
            .filter(|(session, _)| *session == session_id)
            .map_or(0, |(_, passed)| passed))
    }

    async fn record_setup_passed(
        &self,
        key: StepKey,
        session_id: Uuid,
        passed: u16,
    ) -> Result<bool, FakeError> {
        if !self.holds_session(key, session_id).await? {
            return Ok(false);
        }
        let mut sessions = self.sessions.lock().expect("sessions");
        let Some(claimed) = sessions.iter_mut().find(|claimed| claimed.key == key) else {
            return Ok(false);
        };
        claimed.setup = Some((session_id, passed));
        Ok(true)
    }

    async fn stopping_session(
        &self,
        key: StepKey,
        lands_within: std::time::Duration,
    ) -> Result<Option<StoppingSession>, FakeError> {
        let within = chrono::TimeDelta::from_std(lands_within).map_err(|_| FakeError)?;
        let sessions = self.sessions.lock().expect("sessions");
        Ok(sessions
            .iter()
            .find(|claimed| claimed.key == key)
            .map(|claimed| StoppingSession {
                session: StepSession {
                    action_id: claimed.turn.unwrap_or(claimed.session.action_id),
                    ..claimed.session.clone()
                },
                opened: claimed.opened,
                may_still_open: claimed
                    .prepared_at
                    .is_some_and(|at| Utc::now() - at < within),
            }))
    }

    async fn holds_session(&self, key: StepKey, session_id: Uuid) -> Result<bool, FakeError> {
        let held = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .any(|claimed| {
                claimed.key == key && claimed.session.session_id == session_id && !claimed.released
            });
        let run_ended = self
            .runs
            .lock()
            .expect("runs")
            .get(&key.run_id)
            .is_none_or(|run| run.end.is_some());
        Ok(held && !self.step_ended(key) && !run_ended)
    }

    async fn session_step(&self, session_id: Uuid) -> Result<Option<SessionStep>, FakeError> {
        let claimed = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .rev()
            .find(|claimed| claimed.session.session_id == session_id)
            .cloned();
        let Some(claimed) = claimed else {
            return Ok(None);
        };
        let owner = self
            .runs
            .lock()
            .expect("runs")
            .get(&claimed.key.run_id)
            .map(|run| run.user_id.clone())
            .ok_or(FakeError)?;
        Ok(Some(SessionStep {
            key: claimed.key,
            owner,
            offered: claimed.offered,
            ended: self.step_ended(claimed.key),
        }))
    }

    async fn hand_back(&self, session_id: Uuid, hand_back: &HandBack) -> Result<bool, FakeError> {
        let key = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .rev()
            .find(|claimed| claimed.session.session_id == session_id)
            .map(|claimed| claimed.key);
        let Some(key) = key else {
            return Ok(false);
        };
        if self.step_ended(key) {
            return Ok(false);
        }
        let mut sessions = self.sessions.lock().expect("sessions");
        let claimed = sessions.iter_mut().find(|claimed| claimed.key == key);
        let kept = claimed.is_some();
        if let Some(claimed) = claimed {
            claimed.handed = Some(hand_back.clone());
        }
        Ok(kept)
    }

    async fn handed_back(&self, key: StepKey, action_id: Uuid) -> Result<HandedBack, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        let Some(claimed) = sessions.iter().find(|claimed| claimed.key == key) else {
            return Ok(HandedBack::Nothing);
        };
        if claimed.turn.unwrap_or(claimed.session.action_id) != action_id {
            return Ok(HandedBack::Superseded);
        }
        Ok(claimed
            .handed
            .clone()
            .map_or(HandedBack::Nothing, HandedBack::Handed))
    }

    async fn begin_turn(
        &self,
        key: StepKey,
        session_id: Uuid,
        action_id: Uuid,
        place: u16,
        offered: &[Outcome],
    ) -> Result<bool, FakeError> {
        let ended = self.step_ended(key);
        let mut sessions = self.sessions.lock().expect("sessions");
        let Some(claimed) = sessions.iter_mut().find(|claimed| claimed.key == key) else {
            return Ok(false);
        };
        if claimed.session.session_id != session_id || ended {
            return Ok(false);
        }
        let mut recorded_now = false;
        if claimed.turn_place.is_none_or(|recorded| recorded < place) {
            claimed.turn = Some(action_id);
            claimed.turn_place = Some(place);
            claimed.offered = offered.to_vec();
            claimed.handed = None;
            recorded_now = true;
        }
        let began = claimed.turn == Some(action_id);
        drop(sessions);
        if recorded_now {
            self.open_steering(key);
        }
        Ok(began)
    }

    async fn steer(
        &self,
        key: StepKey,
        steer: &NewSteer,
    ) -> Result<Result<Steered, SteerRefusal>, FakeError> {
        self.record_steer(key, steer)
    }

    async fn unsteer(
        &self,
        key: StepKey,
        prompt_id: Uuid,
        previous: Option<Uuid>,
    ) -> Result<(), FakeError> {
        self.undo_steer(key, prompt_id, previous);
        Ok(())
    }

    async fn steering(&self, key: StepKey) -> Result<StepSteering, FakeError> {
        Ok(self.read_steering(key))
    }

    async fn pause_ran_out(&self, key: StepKey) -> Result<(), FakeError> {
        self.record_pause_ran_out(key);
        Ok(())
    }

    async fn close_steering(
        &self,
        key: StepKey,
        turn: Uuid,
        followed: Option<Uuid>,
    ) -> Result<bool, FakeError> {
        Ok(self.stop_steering(key, turn, followed))
    }

    async fn check_rounds(&self, key: StepKey) -> Result<Vec<CheckRound>, FakeError> {
        Ok(self.rounds_of(key))
    }

    async fn start_check_round(
        &self,
        key: StepKey,
        start: &CheckRoundStart,
    ) -> Result<(), FakeError> {
        let mut rounds = self.check_rounds.lock().expect("rounds");
        if !rounds
            .iter()
            .any(|(step, round)| *step == key && round.round == start.round)
        {
            rounds.push((
                key,
                CheckRound {
                    round: start.round,
                    checks: start.checks.clone(),
                    commands: start.commands.clone(),
                    declared: start.declared.clone(),
                    setup: start.setup.clone(),
                    started_at: Utc::now(),
                    end: None,
                    work_output: start.work_output.clone(),
                    check_action_id: start.check_action_id,
                    fix_action_id: None,
                    quota_ran_out: false,
                    verified: false,
                    results: Vec::new(),
                    checked_commit: None,
                },
            ));
        }
        Ok(())
    }

    async fn end_check_round(
        &self,
        key: StepKey,
        round: u8,
        outcome: CheckRoundOutcome,
        report: &str,
    ) -> Result<(), FakeError> {
        let mut rounds = self.check_rounds.lock().expect("rounds");
        if let Some((_, kept)) = rounds
            .iter_mut()
            .find(|(step, kept)| *step == key && kept.round == round)
        {
            kept.end.get_or_insert_with(|| CheckRoundEnd {
                outcome,
                report: report.to_string(),
                ended_at: Utc::now(),
            });
        }
        Ok(())
    }

    async fn record_check_result(
        &self,
        key: StepKey,
        round: u8,
        commit: &str,
        result: &CheckResult,
    ) -> Result<(), FakeError> {
        let mut rounds = self.check_rounds.lock().expect("rounds");
        if let Some((_, kept)) = rounds
            .iter_mut()
            .find(|(step, kept)| *step == key && kept.round == round)
            && kept.end.is_none()
            && !kept.results.iter().any(|run| run.name == result.name)
        {
            kept.verified = true;
            kept.checked_commit
                .get_or_insert_with(|| commit.to_string());
            kept.results.push(result.clone());
        }
        Ok(())
    }

    async fn end_check_round_on_quota(
        &self,
        key: StepKey,
        round: u8,
        report: &str,
    ) -> Result<(), FakeError> {
        let mut rounds = self.check_rounds.lock().expect("rounds");
        if let Some((_, kept)) = rounds
            .iter_mut()
            .find(|(step, kept)| *step == key && kept.round == round)
        {
            let end = kept.end.get_or_insert_with(|| CheckRoundEnd {
                outcome: CheckRoundOutcome::Failed,
                report: report.to_string(),
                ended_at: Utc::now(),
            });
            if end.outcome == CheckRoundOutcome::Failed {
                kept.quota_ran_out = true;
            }
        }
        Ok(())
    }

    async fn send_back_check_round(
        &self,
        key: StepKey,
        round: u8,
        fix_action_id: Uuid,
    ) -> Result<Option<Uuid>, FakeError> {
        let mut rounds = self.check_rounds.lock().expect("rounds");
        let failed = rounds.iter_mut().find(|(step, kept)| {
            *step == key
                && kept.round == round
                && kept
                    .end
                    .as_ref()
                    .is_some_and(|end| end.outcome == CheckRoundOutcome::Failed)
        });
        Ok(failed.map(|(_, kept)| *kept.fix_action_id.get_or_insert(fix_action_id)))
    }

    async fn step_sessions(&self, id: RunId) -> Result<Vec<Uuid>, FakeError> {
        let mut sessions: Vec<Uuid> = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .filter(|claimed| claimed.key.run_id == id)
            .flat_map(|claimed| {
                claimed
                    .left
                    .iter()
                    .copied()
                    .chain([claimed.session.session_id])
            })
            .collect();
        sessions.sort();
        sessions.dedup();
        Ok(sessions)
    }

    async fn record_budget_stop(&self, id: RunId, stop: BudgetStop) -> Result<(), FakeError> {
        if let Some(run) = self.runs.lock().expect("runs").get_mut(&id) {
            let budgeted = run
                .standing_rules
                .is_some_and(|rules| rules.definition.token_budget.is_some());
            if budgeted && run.budget_stop.is_none() {
                run.budget_stop = Some(stop);
            }
        }
        Ok(())
    }

    async fn record_cancel_sender(&self, id: RunId, in_person: bool) -> Result<(), FakeError> {
        let open = self
            .runs
            .lock()
            .expect("runs")
            .get(&id)
            .is_some_and(|run| run.end.is_none());
        if open {
            *self
                .cancelled_in_person
                .lock()
                .expect("cancelled in person")
                .entry(id)
                .or_default() |= in_person;
        }
        Ok(())
    }

    async fn end_run(&self, id: RunId, outcome: RunOutcome) -> Result<(), FakeError> {
        if let Some(run) = self.runs.lock().expect("runs").get_mut(&id)
            && run.end.is_none()
        {
            run.end = Some(RunEnd {
                outcome,
                ended_at: Utc::now(),
                flaw: None,
            });
            // A cancelled Run's open Steps end with it.
            if outcome == RunOutcome::Cancelled {
                let cancelled =
                    Outcome::try_from("cancelled".to_string()).map_err(|_| FakeError)?;
                let mut steps = self.steps.lock().expect("steps");
                for (_, step) in steps.iter_mut().filter(|(run, _)| *run == id) {
                    step.end.get_or_insert_with(|| StepEnd {
                        outcome: cancelled.clone(),
                        output: "The run was cancelled.".to_string(),
                        ended_at: Utc::now(),
                    });
                }
            }
        }
        // As the database: a Run that ended holds no place in a queue.
        self.queue().queued.retain(|queued| queued.run_id != id);
        Ok(())
    }
}

/// What [`FakeGithub`] holds: one repository, and the pull request a Run's
/// branch opens in it.
#[derive(Default)]
pub(crate) struct FakeGithubState {
    /// The CI jobs that finished on a commit, as CI health reads them,
    /// unless [`Self::finished_on`] names that commit.
    pub(crate) finished: Vec<FinishedJob>,
    /// The CI jobs that finished on each commit named here.
    pub(crate) finished_on: BTreeMap<String, Vec<FinishedJob>>,
    /// Whether reading them fails, as an outage would.
    pub(crate) finished_fails: bool,
    /// How long reading them takes, as a slow GitHub would.
    pub(crate) finished_delay: Option<std::time::Duration>,
    /// The commits whose finished jobs were read, in order.
    pub(crate) finished_reads: Vec<String>,
    /// Whether each read of finished jobs asked for details, in order.
    pub(crate) finished_details: Vec<bool>,
    /// The commit each read of the open pull request finds its head at
    /// next, in order; the last one stays. Empty keeps its head.
    pub(crate) heads: VecDeque<String>,
    /// Whether the Run's owner cannot reach the repository.
    pub(crate) unreachable: bool,
    /// Whether the Run's owner may only read the repository.
    pub(crate) read_only: bool,
    /// The checks the base branch requires, when they can be named.
    pub(crate) required: Option<RequiredChecks>,
    /// The pull request from the Run's branch, once opened.
    pub(crate) pull_request: Option<PullRequest>,
    /// The checks each read of the open pull request's head shows next, in
    /// order; the last one stays once the others were read.
    pub(crate) checks: VecDeque<HeadChecks>,
    /// How many times the pull request was looked for.
    pub(crate) reads: usize,
    /// How many times a pull request was opened.
    pub(crate) opened: usize,
    /// GitHub's reason to refuse opening one, if it does.
    pub(crate) open_refusal: Option<String>,
    /// The descriptions pull requests were opened with.
    pub(crate) bodies: Vec<String>,
    /// The head commits merges were asked for, in order.
    pub(crate) merges: Vec<String>,
    /// GitHub's reason to refuse a merge, if it does.
    pub(crate) merge_refusal: Option<String>,
    /// The text of each file, by its branch and path.
    pub(crate) files: BTreeMap<(String, String), String>,
    /// The files read, by their branch and path, in order.
    pub(crate) file_reads: Vec<(String, String)>,
    /// The paths the Run's branch changed against its base; none when
    /// GitHub cannot list them all.
    pub(crate) changed: Option<Vec<String>>,
    /// The branches or commits whose changes were asked for, in order.
    pub(crate) compared: Vec<String>,
    /// The merge base GitHub gives of a comparison a round's checks are
    /// told of; none for a commit no comparison names.
    pub(crate) check_base: Option<String>,
    /// Whether comparing for a round's checks fails, as GitHub down does.
    pub(crate) compare_fails: bool,
    /// The bases and heads compared for a round's checks, in order.
    pub(crate) compared_changes: Vec<(String, String)>,
    /// The base branch's head commit and the checks on it; none when the
    /// owner reaches no such branch.
    pub(crate) base: Option<(String, HeadChecks)>,
    /// Whether reads of the base branch fail, as GitHub down does.
    pub(crate) base_unreadable: bool,
    /// The owners GitHub refuses, as when their link no longer works.
    pub(crate) refused_owners: BTreeSet<String>,
    /// When set, each read of the base branch waits to be let go.
    pub(crate) base_gate: Option<Arc<tokio::sync::Semaphore>>,
    /// The commit the Run's branch is at, each time it is read next, in
    /// order; the last one stays. None when the branch is not there.
    pub(crate) branch_heads: VecDeque<Option<String>>,
    /// The files each diff changed, by its base and head; a diff not here
    /// cannot be listed in full.
    pub(crate) diffs: BTreeMap<(String, String), Vec<ChangedFile>>,
    /// The head commit of the Run's branch; none before it is pushed.
    pub(crate) head: Option<String>,
    /// Whether GitHub deletes a pull request's branch once it merges.
    pub(crate) deletes_merged_branches: bool,
    /// The branches deleted, in order.
    pub(crate) deleted_branches: Vec<String>,
    /// Whether deleting a branch fails, as GitHub down does.
    pub(crate) delete_fails: bool,
    /// Whether an open pull request is based on the Run's branch.
    pub(crate) based_on_branch: bool,
    /// Whether asking for pull requests based on a branch fails, as GitHub
    /// down does.
    pub(crate) onto_fails: bool,
    /// The branches pull requests based on them were asked for, in order.
    pub(crate) onto_asked: Vec<String>,
    /// How many times what the owner may do with the repository was read.
    pub(crate) access_reads: usize,
    /// Whether reading what the owner may do with the repository fails.
    pub(crate) access_fails: bool,
    /// Whether reading the head of the Run's branch fails.
    pub(crate) branch_head_fails: bool,
    /// Whether finding a branch's pull request fails, as GitHub down does.
    pub(crate) find_fails: bool,
    /// When set, each deletion of a branch waits to be let go.
    pub(crate) delete_gate: Option<Arc<tokio::sync::Semaphore>>,
    /// Whether the default branch takes changes only through a pull
    /// request; none when that cannot be told.
    pub(crate) requires_pull_request: Option<bool>,
    /// The `owner/name` GitHub answers a repository with, when it says.
    pub(crate) full_name: Option<String>,
    /// The paths of the files GitHub will not give, as one over 1 MB.
    pub(crate) refused_files: BTreeSet<String>,
    /// The pairs of commits whose second contains the first.
    pub(crate) descendants: BTreeSet<(String, String)>,
    /// The merge base of two commits neither contains the other of.
    pub(crate) merge_bases: BTreeMap<(String, String), String>,
    /// The parents of each commit, by its id; a commit not listed is none
    /// GitHub has.
    pub(crate) parents: BTreeMap<String, Vec<String>>,
    /// The commits of `parents` someone made by hand; GitHub made the
    /// others itself.
    pub(crate) made_by_hand: BTreeSet<String>,
    /// The head commit of each branch that is not a Run's, by its name.
    pub(crate) other_heads: BTreeMap<String, String>,
    /// The heads updates of the Run's branch were asked from, in order.
    pub(crate) updates: Vec<String>,
    /// What GitHub answers an update of the Run's branch with; `Started`
    /// when none is set, which moves the pull request's and the branch's
    /// head to [`Self::updated_head`].
    pub(crate) update_answer: Option<BranchUpdating>,
    /// The head GitHub's merge of the base makes, once it merged.
    pub(crate) updated_head: Option<String>,
    /// The commit `main` moves to as a merge is asked, as when another
    /// merge lands just before it.
    pub(crate) main_moves_on_merge: Option<String>,
}

/// GitHub in memory, for one Run's branch: opening its pull request makes
/// it listed, open, at [`FakeGithub::HEAD`], and merging it merges it.
#[derive(Clone, Default)]
pub(crate) struct FakeGithub {
    pub(crate) state: Arc<Mutex<FakeGithubState>>,
    /// Told each time the pull request is looked for.
    pub(crate) read: Arc<tokio::sync::Notify>,
    /// Told each time the base branch's checks are read.
    pub(crate) base_read: Arc<tokio::sync::Notify>,
}

impl FakeGithub {
    /// The commit an opened pull request's head is at.
    pub(crate) const HEAD: &str = "1111111111111111111111111111111111111111";

    /// GitHub's id of the CI app whose runs [`ci`] makes.
    pub(crate) const ACTIONS: u64 = 15368;

    pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, FakeGithubState> {
        self.state.lock().expect("github")
    }

    /// GitHub with the Run's pull request open at [`Self::HEAD`], its head
    /// showing `checks`.
    pub(crate) fn with_open_pull_request(checks: HeadChecks) -> Self {
        let github = Self::default();
        github.state().pull_request = Some(PullRequest {
            number: 7,
            url: "https://github.com/macro/app/pull/7".to_string(),
            state: PullRequestState::Open,
            head_sha: Self::HEAD.to_string(),
        });
        github.state().checks = [checks].into();
        github
    }

    /// The pull request, with its head's next checks.
    fn read(&self) -> Option<(PullRequest, HeadChecks)> {
        let found = {
            let mut state = self.state();
            state.reads += 1;
            let checks = if state.checks.len() > 1 {
                state.checks.pop_front()
            } else {
                state.checks.front().cloned()
            };
            let head = if state.heads.len() > 1 {
                state.heads.pop_front()
            } else {
                state.heads.front().cloned()
            };
            if let (Some(head), Some(pull_request)) = (head, state.pull_request.as_mut()) {
                pull_request.head_sha = head;
            }
            state
                .pull_request
                .clone()
                .map(|pull_request| (pull_request, checks.unwrap_or_default()))
        };
        self.read.notify_one();
        found
    }
}

/// One suite of `app`, holding a run: completed with `conclusion`, or still
/// running when `conclusion` is none.
pub(crate) fn suite(id: u64, app: &str, conclusion: Option<&str>) -> CheckSuite {
    CheckSuite {
        id,
        app: app.to_string(),
        state: match conclusion {
            Some(conclusion) => CheckState::of_check("completed", Some(conclusion)),
            None => CheckState::Waiting,
        },
        check_runs: 1,
        created_at: Some(Utc::now()),
    }
}

/// A head whose checks are the CI suite with its one run, `test`,
/// concluded `conclusion`, or running when it is none.
pub(crate) fn ci(conclusion: Option<&str>) -> HeadChecks {
    let state = match conclusion {
        Some(conclusion) => CheckState::of_check("completed", Some(conclusion)),
        None => CheckState::Waiting,
    };
    HeadChecks {
        suites: vec![suite(1, "github-actions", conclusion)],
        statuses: Vec::new(),
        runs: vec![Check {
            name: "test".to_string(),
            state,
            url: Some("https://github.com/macro/app/runs/test".to_string()),
            app_id: Some(FakeGithub::ACTIONS),
        }],
        truncated: false,
        held_apps: Vec::new(),
        actions_unread: false,
    }
}

impl PullRequests for FakeGithub {
    fn file_refusal(error: &FakeGithubError) -> Option<FileRefusal> {
        match error {
            FakeGithubError::TooLarge => Some(FileRefusal::TooLarge),
            FakeGithubError::Refused => Some(FileRefusal::Refused),
            FakeGithubError::Down => None,
        }
    }

    type Err = FakeGithubError;

    async fn repository_access(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
    ) -> Result<Option<RepositoryAccess>, FakeGithubError> {
        let mut state = self.state();
        state.access_reads += 1;
        if state.access_fails {
            return Err(FakeGithubError::Down);
        }
        Ok((!state.unreachable).then(|| RepositoryAccess {
            full_name: state.full_name.clone(),
            default_branch: "main".to_string(),
            can_push: !state.read_only,
            deletes_merged_branches: state.deletes_merged_branches,
        }))
    }

    async fn find(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
    ) -> Result<Option<PullRequest>, FakeGithubError> {
        if self.state().find_fails {
            return Err(FakeGithubError::Down);
        }
        Ok(self.read().map(|(pull_request, _)| pull_request))
    }

    async fn find_with_checks(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
        _all_runs: bool,
    ) -> Result<Option<(PullRequest, HeadChecks)>, FakeGithubError> {
        Ok(self.read())
    }

    async fn finished_jobs(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        sha: &str,
        details: bool,
    ) -> Result<Vec<FinishedJob>, FakeGithubError> {
        let (delay, read) = {
            let mut state = self.state();
            state.finished_reads.push(sha.to_string());
            state.finished_details.push(details);
            let read = if state.finished_fails {
                Err(FakeGithubError::Down)
            } else {
                Ok(state
                    .finished_on
                    .get(sha)
                    .cloned()
                    .unwrap_or_else(|| state.finished.clone()))
            };
            (state.finished_delay, read)
        };
        if let Some(delay) = delay {
            tokio::time::sleep(delay).await;
        }
        read
    }

    async fn branch_checks(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
        _all_runs: bool,
    ) -> Result<Option<(String, HeadChecks)>, FakeGithubError> {
        let gate = self.state().base_gate.clone();
        self.base_read.notify_one();
        if let Some(gate) = gate {
            gate.acquire().await.expect("gate").forget();
        }
        let state = self.state();
        if state.base_unreadable {
            return Err(FakeGithubError::Down);
        }
        Ok(state.base.clone())
    }

    async fn required_checks(
        &self,
        owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _base: &str,
    ) -> Result<Option<RequiredChecks>, FakeGithubError> {
        let state = self.state();
        if state.base_unreadable {
            return Err(FakeGithubError::Down);
        }
        if state.refused_owners.contains(owner.as_ref()) {
            return Err(FakeGithubError::Refused);
        }
        Ok(state.required.clone())
    }

    async fn requires_pull_request(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
    ) -> Result<Option<bool>, FakeGithubError> {
        Ok(self.state().requires_pull_request)
    }

    async fn file_text(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        branch: &str,
        path: &str,
    ) -> Result<Option<String>, FakeGithubError> {
        let file = (branch.to_string(), path.to_string());
        let mut state = self.state();
        state.file_reads.push(file.clone());
        if state.refused_files.contains(path) {
            return Err(FakeGithubError::TooLarge);
        }
        Ok(state.files.get(&file).cloned())
    }

    async fn changed_paths(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _base: &str,
        branch: &str,
    ) -> Result<Option<Vec<String>>, FakeGithubError> {
        let mut state = self.state();
        state.compared.push(branch.to_string());
        // Once merged, the base holds the head: GitHub lists no change.
        let merged = state
            .pull_request
            .as_ref()
            .is_some_and(|pull_request| pull_request.state == PullRequestState::Merged);
        if merged {
            return Ok(Some(Vec::new()));
        }
        Ok(state.changed.clone())
    }

    async fn compare_changes(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        base: &str,
        head: &str,
    ) -> Result<ComparedChanges, FakeGithubError> {
        let mut state = self.state();
        state
            .compared_changes
            .push((base.to_string(), head.to_string()));
        if state.compare_fails {
            return Err(FakeGithubError::Down);
        }
        Ok(ComparedChanges {
            merge_base: state.check_base.clone().unwrap_or_default(),
            paths: state.changed.clone().unwrap_or_default(),
            complete: state.changed.is_some(),
        })
    }

    async fn branch_head(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
    ) -> Result<Option<String>, FakeGithubError> {
        let mut state = self.state();
        if state.branch_head_fails {
            return Err(FakeGithubError::Down);
        }
        if let Some(head) = state.other_heads.get(_branch) {
            return Ok(Some(head.clone()));
        }
        if state.branch_heads.is_empty() {
            return Ok(state.head.clone());
        }
        let head = if state.branch_heads.len() > 1 {
            state.branch_heads.pop_front()
        } else {
            state.branch_heads.front().cloned()
        };
        Ok(head.flatten())
    }

    async fn changed_files(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        base: &str,
        head: &str,
    ) -> Result<Option<Vec<ChangedFile>>, FakeGithubError> {
        let diff = (base.to_string(), head.to_string());
        Ok(self.state().diffs.get(&diff).cloned())
    }

    async fn open(
        &self,
        _owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        pull_request: &OpenPullRequest,
    ) -> Result<Opening, FakeGithubError> {
        let mut state = self.state();
        if let Some(why) = state.open_refusal.clone() {
            return Ok(Opening::Refused(why));
        }
        state.opened += 1;
        state.bodies.push(pull_request.body.clone());
        state.pull_request = Some(PullRequest {
            number: 7,
            url: format!("{}/pull/7", repository.url()),
            state: PullRequestState::Open,
            head_sha: Self::HEAD.to_string(),
        });
        Ok(Opening::Opened)
    }

    async fn merge(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _number: u64,
        head_sha: &str,
    ) -> Result<Merging, FakeGithubError> {
        let mut state = self.state();
        state.merges.push(head_sha.to_string());
        if let Some(moved) = state.main_moves_on_merge.take() {
            state.other_heads.insert("main".to_string(), moved);
        }
        if let Some(why) = state.merge_refusal.clone() {
            return Ok(Merging::Refused(why));
        }
        let Some(pull_request) = state.pull_request.as_mut() else {
            return Ok(Merging::Refused("Not Found".to_string()));
        };
        if pull_request.head_sha != head_sha {
            return Ok(Merging::HeadChanged);
        }
        pull_request.state = PullRequestState::Merged;
        Ok(Merging::Merged)
    }

    async fn update_branch(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _number: u64,
        expected_head: &str,
    ) -> Result<BranchUpdating, FakeGithubError> {
        let mut state = self.state();
        state.updates.push(expected_head.to_string());
        if let Some(answer) = state.update_answer.clone() {
            return Ok(answer);
        }
        let head = state
            .pull_request
            .as_ref()
            .map(|pull_request| pull_request.head_sha.clone());
        if head.as_deref() != Some(expected_head) {
            return Ok(BranchUpdating::HeadMoved);
        }
        if let Some(updated) = state.updated_head.clone() {
            if let Some(pull_request) = state.pull_request.as_mut() {
                pull_request.head_sha = updated.clone();
            }
            state.branch_heads.clear();
            state.head = Some(updated);
        }
        Ok(BranchUpdating::Started)
    }

    async fn has_open_pull_request_onto(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        branch: &str,
    ) -> Result<bool, FakeGithubError> {
        let mut state = self.state();
        state.onto_asked.push(branch.to_string());
        if state.onto_fails {
            return Err(FakeGithubError::Down);
        }
        Ok(state.based_on_branch)
    }

    /// Deleting the Run's branch leaves it not there for later reads.
    async fn delete_run_branch(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        run_id: RunId,
    ) -> Result<BranchDeletion, FakeGithubError> {
        let gate = self.state().delete_gate.clone();
        if let Some(gate) = gate {
            gate.acquire().await.expect("gate").forget();
        }
        let mut state = self.state();
        if state.delete_fails {
            return Err(FakeGithubError::Down);
        }
        let there = state.head.is_some() || state.branch_heads.iter().any(Option::is_some);
        state.head = None;
        state.branch_heads = [None].into();
        if !there {
            return Ok(BranchDeletion::Missing);
        }
        state.deleted_branches.push(run_branch(run_id));
        Ok(BranchDeletion::Deleted)
    }

    async fn commit_made(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        sha: &str,
    ) -> Result<Option<CommitMade>, FakeGithubError> {
        let state = self.state();
        let by_hand = state.made_by_hand.contains(sha);
        Ok(state.parents.get(sha).map(|parents| CommitMade {
            parents: parents.clone(),
            // A commit made by hand may name GitHub's email, and so its
            // account, as its committer, but GitHub verifies no signature.
            committer_login: Some(GITHUB_COMMITTER.to_string()),
            verified: !by_hand,
        }))
    }

    async fn merge_base(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        base: &str,
        head: &str,
    ) -> Result<Option<String>, FakeGithubError> {
        let state = self.state();
        let (base, head) = (base.to_string(), head.to_string());
        if base == head || state.descendants.contains(&(base.clone(), head.clone())) {
            return Ok(Some(base));
        }
        if state.descendants.contains(&(head.clone(), base.clone())) {
            return Ok(Some(head));
        }
        // Commits neither of which contains the other share only the base
        // registered for them, or no history.
        Ok(state
            .merge_bases
            .get(&(base.clone(), head.clone()))
            .or_else(|| state.merge_bases.get(&(head, base)))
            .cloned())
    }
}

impl StandingRulesRepository for FakeRepo {
    type Err = FakeError;

    async fn team_rules(&self, team_id: Uuid) -> Result<Option<OrchestratorDefinition>, FakeError> {
        Ok(self
            .team_rules
            .lock()
            .expect("rules")
            .get(&team_id)
            .copied())
    }

    async fn set_team_rules(
        &self,
        team_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> Result<(), FakeError> {
        let mut kept = self.team_rules.lock().expect("rules");
        match definition {
            Some(definition) => kept.insert(team_id, definition),
            None => kept.remove(&team_id),
        };
        Ok(())
    }

    async fn project_rules(
        &self,
        team_id: Uuid,
        project_id: Uuid,
    ) -> Result<Option<OrchestratorDefinition>, FakeError> {
        Ok(self
            .project_rules
            .lock()
            .expect("rules")
            .get(&(team_id, project_id))
            .copied())
    }

    async fn list_project_rules(&self, team_id: Uuid) -> Result<Vec<ProjectRules>, FakeError> {
        Ok(self
            .project_rules
            .lock()
            .expect("rules")
            .iter()
            .filter(|((team, _), _)| *team == team_id)
            .map(|((_, project_id), definition)| ProjectRules {
                project_id: *project_id,
                definition: *definition,
            })
            .collect())
    }

    async fn set_project_rules(
        &self,
        team_id: Uuid,
        project_id: Uuid,
        definition: Option<OrchestratorDefinition>,
    ) -> Result<ProjectRulesSet, FakeError> {
        let mut kept = self.project_rules.lock().expect("rules");
        match definition {
            None => {
                kept.remove(&(team_id, project_id));
            }
            Some(_)
                if !self
                    .projects
                    .lock()
                    .expect("projects")
                    .contains(&project_id) =>
            {
                return Ok(ProjectRulesSet::NoSuchProject);
            }
            Some(definition) => {
                kept.insert((team_id, project_id), definition);
            }
        }
        Ok(ProjectRulesSet::Set)
    }
}
