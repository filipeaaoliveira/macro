//! In-memory stand-ins for the factory's records, for the domain's tests.

mod gate_owner;
mod gate_replies;
mod main_hold;

pub(crate) use gate_owner::{FakeNotifier, FakeTaskNotes};
pub(crate) use gate_replies::FakeGateModel;
pub(crate) use main_hold::{FakeHoldTasks, FakeMain, FakeWatches};

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use agent_session::domain::session_task::TaskDocumentId;
use chrono::Utc;
use factory_records::domain::models::{
    CheckResult, CheckRound, CheckRoundEnd, CheckRoundOutcome, CheckRoundStart, ClaimedSession,
    ControlFileChanges, EntryId, Gate, GateAnswer, GateKind, HandBack, HandedBack, NewRun, Outcome,
    PassedOver, Role, Run, RunEnd, RunId, RunInsert, RunOutcome, SessionStep, Step, StepEnd,
    StepKey, StepSession, StoppingSession, TaskLink, TeamMembership,
};
use factory_records::domain::ports::RunRepository;
use factory_records::domain::standing_rules::{
    OrchestratorDefinition, ProjectRules, ProjectRulesSet, StandingRule, StandingRulesRepository,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{
    Check, CheckState, CheckSuite, CodeRepository, HeadChecks, Merging, OpenPullRequest, Opening,
    PullRequest, PullRequestState, RepositoryAccess, RequiredChecks,
};
use crate::domain::ports::{
    FollowUpTask, PullRequests, Retryable, TaskAccess, TaskDirectory, UserDirectory,
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
    /// The built-in Flow each Run walks.
    pub(crate) flows: Arc<Mutex<BTreeMap<RunId, String>>>,
    pub(crate) steps: Arc<Mutex<Vec<(RunId, Step)>>>,
    /// Each agent Step's session, what it offers, and its latest hand back.
    pub(crate) sessions: Arc<Mutex<Vec<FakeSession>>>,
    /// Each Step's rounds of checks, in order.
    pub(crate) check_rounds: Arc<Mutex<Vec<(StepKey, CheckRound)>>>,
    /// The task documents that exist.
    pub(crate) tasks: Arc<Mutex<BTreeSet<Uuid>>>,
    /// The documents that exist and are not tasks.
    pub(crate) documents: Arc<Mutex<BTreeSet<Uuid>>>,
    /// The task documents a user cannot see: their id and the task's.
    pub(crate) hidden: Arc<Mutex<BTreeSet<(String, Uuid)>>>,
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
    /// The team each person is on, by their id.
    pub(crate) teams: Arc<Mutex<BTreeMap<String, TeamMembership>>>,
    /// The Control File patterns each team set.
    pub(crate) control_files: Arc<Mutex<BTreeMap<Uuid, Vec<String>>>>,
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
}

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
            repository,
            created_in_person,
            control_file_patterns,
            control_files_team_id,
            standing_rules,
            ..
        } = new;
        if let Some(task_id) = task_id
            && !self.tasks.lock().expect("tasks").contains(&task_id)
        {
            return Ok(RunInsert::NoSuchTask);
        }
        let run = Run {
            id,
            request: request.to_string(),
            user_id: user_id.clone(),
            created_at: Utc::now(),
            task_id,
            repository: repository.cloned(),
            created_in_person,
            control_file_patterns: Some(control_file_patterns.to_vec()),
            control_files_team_id,
            control_files: None,
            standing_rules: Some(standing_rules),
            end: None,
        };
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
    ) -> Result<(), FakeError> {
        self.with_step(key, |step| {
            if let Some(gate) = step.gate.as_mut()
                && gate.answer.is_none()
            {
                gate.answer = Some(GateAnswer {
                    reply_confidence: None,
                    by_stand_in: false,
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
            claimed.session = to.clone();
            claimed.prepared_at = None;
            claimed.opened = false;
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
        if claimed.turn_place.is_none_or(|recorded| recorded < place) {
            claimed.turn = Some(action_id);
            claimed.turn_place = Some(place);
            claimed.offered = offered.to_vec();
            claimed.handed = None;
        }
        Ok(claimed.turn == Some(action_id))
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

    async fn end_run(&self, id: RunId, outcome: RunOutcome) -> Result<(), FakeError> {
        if let Some(run) = self.runs.lock().expect("runs").get_mut(&id)
            && run.end.is_none()
        {
            run.end = Some(RunEnd {
                outcome,
                ended_at: Utc::now(),
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
        Ok(())
    }
}

/// What [`FakeGithub`] holds: one repository, and the pull request a Run's
/// branch opens in it.
#[derive(Default)]
pub(crate) struct FakeGithubState {
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
    /// The paths the Run's branch changed against its base; none when
    /// GitHub cannot list them all.
    pub(crate) changed: Option<Vec<String>>,
    /// The branches or commits whose changes were asked for, in order.
    pub(crate) compared: Vec<String>,
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
    type Err = FakeGithubError;

    async fn repository_access(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
    ) -> Result<Option<RepositoryAccess>, FakeGithubError> {
        let state = self.state();
        Ok((!state.unreachable).then(|| RepositoryAccess {
            default_branch: "main".to_string(),
            can_push: !state.read_only,
        }))
    }

    async fn find(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
    ) -> Result<Option<PullRequest>, FakeGithubError> {
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

    async fn file_text(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        branch: &str,
        path: &str,
    ) -> Result<Option<String>, FakeGithubError> {
        let file = (branch.to_string(), path.to_string());
        Ok(self.state().files.get(&file).cloned())
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

    async fn branch_head(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
    ) -> Result<Option<String>, FakeGithubError> {
        let mut state = self.state();
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
