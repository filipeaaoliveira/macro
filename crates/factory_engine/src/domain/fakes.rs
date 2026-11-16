//! In-memory stand-ins for the factory's records, for the domain's tests.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

use agent_session::domain::session_task::TaskDocumentId;
use chrono::Utc;
use factory_records::domain::models::{
    ClaimedSession, EntryId, Gate, HandBack, Outcome, Repository, Run, RunEnd, RunId, RunInsert,
    RunOutcome, SessionStep, Step, StepEnd, StepKey, StepSession, StoppingSession, TaskLink,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{
    Check, CodeRepository, Merging, OpenPullRequest, Opening, PullRequest, PullRequestState,
};
use crate::domain::ports::{PullRequests, Retryable, TaskAccess, TaskDirectory, UserDirectory};

/// The session an agent Step claimed, and what it handed back.
#[derive(Clone)]
pub(crate) struct FakeSession {
    pub(crate) key: StepKey,
    pub(crate) session: StepSession,
    pub(crate) offered: Vec<Outcome>,
    pub(crate) handed: Option<HandBack>,
    pub(crate) released: bool,
    /// When an attempt last asked for the session to be opened.
    pub(crate) prepared_at: Option<chrono::DateTime<Utc>>,
    pub(crate) opened: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("fake failure")]
pub(crate) struct FakeError;

/// A fake's failure is the transport's: worth trying again.
impl Retryable for FakeError {
    fn retryable(&self) -> bool {
        true
    }
}

/// Runs, Steps and sessions in memory, with the same first-write-wins rules
/// as the Postgres repository.
#[derive(Clone, Default)]
pub(crate) struct FakeRepo {
    pub(crate) runs: Arc<Mutex<BTreeMap<RunId, Run>>>,
    pub(crate) steps: Arc<Mutex<Vec<(RunId, Step)>>>,
    /// Each agent Step's session, what it offers, and its latest hand back.
    pub(crate) sessions: Arc<Mutex<Vec<FakeSession>>>,
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
}

impl FakeRepo {
    fn with_step<T>(&self, key: StepKey, change: impl FnOnce(&mut Step) -> T) -> Option<T> {
        let mut steps = self.steps.lock().expect("steps");
        steps
            .iter_mut()
            .find(|(run, step)| *run == key.run_id && step.seq == key.seq)
            .map(|(_, step)| change(step))
    }

    fn step_ended(&self, key: StepKey) -> bool {
        self.with_step(key, |step| step.end.is_some())
            .unwrap_or(false)
    }
}

impl RunRepository for FakeRepo {
    type Err = FakeError;

    async fn insert_run(
        &self,
        id: RunId,
        _flow: &str,
        request: &str,
        user_id: &MacroUserIdStr<'static>,
        task_id: Option<Uuid>,
        repository: Option<&Repository>,
    ) -> Result<RunInsert, FakeError> {
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
            end: None,
        };
        self.runs.lock().expect("runs").insert(id, run.clone());
        Ok(RunInsert::Inserted(run))
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
        Ok(self.runs.lock().expect("runs").get(&id).cloned())
    }

    async fn list_steps(&self, id: RunId) -> Result<Vec<Step>, FakeError> {
        let steps = self.steps.lock().expect("steps");
        let mut steps: Vec<Step> = steps
            .iter()
            .filter(|(run, _)| *run == id)
            .map(|(_, step)| step.clone())
            .collect();
        steps.sort_by_key(|step| step.seq);
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
            step.gate.get_or_insert_with(|| Gate {
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
        _key: StepKey,
        _answer: &Outcome,
        _note: Option<&str>,
        _user_id: &MacroUserIdStr<'static>,
    ) -> Result<(), FakeError> {
        Ok(())
    }

    async fn claim_session(
        &self,
        key: StepKey,
        session: StepSession,
        offered: &[Outcome],
    ) -> Result<ClaimedSession, FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions.iter().find(|claimed| claimed.key == key) {
            return Ok(ClaimedSession {
                session: claimed.session,
                new: false,
            });
        }
        sessions.push(FakeSession {
            key,
            session,
            offered: offered.to_vec(),
            handed: None,
            released: false,
            prepared_at: None,
            opened: false,
        });
        Ok(ClaimedSession { session, new: true })
    }

    async fn claimed_session(&self, key: StepKey) -> Result<Option<StepSession>, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        Ok(sessions
            .iter()
            .find(|claimed| claimed.key == key)
            .map(|claimed| claimed.session))
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
        let claimed = sessions
            .iter_mut()
            .find(|claimed| claimed.key == key && !claimed.released);
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

    async fn record_open(&self, key: StepKey) -> Result<(), FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions.iter_mut().find(|claimed| claimed.key == key) {
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
                session: claimed.session,
                opened: claimed.opened,
                may_still_open: claimed
                    .prepared_at
                    .is_some_and(|at| Utc::now() - at < within),
            }))
    }

    async fn holds_session(&self, key: StepKey) -> Result<bool, FakeError> {
        let held = self
            .sessions
            .lock()
            .expect("sessions")
            .iter()
            .any(|claimed| claimed.key == key && !claimed.released);
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

    async fn handed_back(&self, key: StepKey) -> Result<Option<HandBack>, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        Ok(sessions
            .iter()
            .find(|claimed| claimed.key == key)
            .and_then(|claimed| claimed.handed.clone()))
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
    /// The pull request from the Run's branch, once opened.
    pub(crate) pull_request: Option<PullRequest>,
    /// The checks each read of the open pull request shows next, in order;
    /// the last one stays once the others were read.
    pub(crate) checks: VecDeque<Vec<Check>>,
    /// How many times the pull request was looked for.
    pub(crate) reads: usize,
    /// How many times a pull request was opened.
    pub(crate) opened: usize,
    /// GitHub's reason to refuse opening one, if it does.
    pub(crate) open_refusal: Option<String>,
    /// The head commits merges were asked for, in order.
    pub(crate) merges: Vec<String>,
    /// GitHub's reason to refuse a merge, if it does.
    pub(crate) merge_refusal: Option<String>,
}

/// GitHub in memory, for one Run's branch: opening its pull request makes
/// it listed, open, at [`FakeGithub::HEAD`], and merging it merges it.
#[derive(Clone, Default)]
pub(crate) struct FakeGithub {
    pub(crate) state: Arc<Mutex<FakeGithubState>>,
    /// Told each time the pull request is looked for.
    pub(crate) read: Arc<tokio::sync::Notify>,
}

impl FakeGithub {
    /// The commit an opened pull request's head is at.
    pub(crate) const HEAD: &str = "1111111111111111111111111111111111111111";

    pub(crate) fn state(&self) -> std::sync::MutexGuard<'_, FakeGithubState> {
        self.state.lock().expect("github")
    }

    /// GitHub with the Run's pull request open at [`Self::HEAD`], showing
    /// `checks`.
    pub(crate) fn with_open_pull_request(checks: Vec<Check>) -> Self {
        let github = Self::default();
        github.state().pull_request = Some(PullRequest {
            number: 7,
            url: "https://github.com/macro/app/pull/7".to_string(),
            state: PullRequestState::Open,
            head_sha: Some(Self::HEAD.to_string()),
            checks: Some(checks),
        });
        github
    }
}

/// A completed check named `name` that concluded `conclusion`, or one still
/// running when `conclusion` is none.
pub(crate) fn check(name: &str, conclusion: Option<&str>) -> Check {
    Check {
        name: name.to_string(),
        status: if conclusion.is_some() {
            "completed"
        } else {
            "in_progress"
        }
        .to_string(),
        conclusion: conclusion.map(str::to_string),
        url: Some(format!("https://github.com/macro/app/runs/{name}")),
    }
}

impl PullRequests for FakeGithub {
    type Err = FakeError;

    async fn default_branch(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
    ) -> Result<Option<String>, FakeError> {
        Ok((!self.state().unreachable).then(|| "main".to_string()))
    }

    async fn find(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _branch: &str,
    ) -> Result<Option<PullRequest>, FakeError> {
        let found = {
            let mut state = self.state();
            state.reads += 1;
            if state.pull_request.is_some() {
                let next = if state.checks.len() > 1 {
                    state.checks.pop_front()
                } else {
                    state.checks.front().cloned()
                };
                if let (Some(pull_request), Some(checks)) = (state.pull_request.as_mut(), next) {
                    pull_request.checks = Some(checks);
                }
            }
            state.pull_request.clone()
        };
        self.read.notify_one();
        Ok(found)
    }

    async fn open(
        &self,
        _owner: &MacroUserIdStr<'static>,
        repository: &CodeRepository,
        _pull_request: &OpenPullRequest,
    ) -> Result<Opening, FakeError> {
        let mut state = self.state();
        if let Some(why) = state.open_refusal.clone() {
            return Ok(Opening::Refused(why));
        }
        state.opened += 1;
        state.pull_request = Some(PullRequest {
            number: 7,
            url: format!("{}/pull/7", repository.url()),
            state: PullRequestState::Open,
            head_sha: Some(Self::HEAD.to_string()),
            checks: Some(Vec::new()),
        });
        Ok(Opening::Opened)
    }

    async fn merge(
        &self,
        _owner: &MacroUserIdStr<'static>,
        _repository: &CodeRepository,
        _number: u64,
        head_sha: &str,
    ) -> Result<Merging, FakeError> {
        let mut state = self.state();
        state.merges.push(head_sha.to_string());
        if let Some(why) = state.merge_refusal.clone() {
            return Ok(Merging::Refused(why));
        }
        let Some(pull_request) = state.pull_request.as_mut() else {
            return Ok(Merging::Refused("Not Found".to_string()));
        };
        if pull_request.head_sha.as_deref() != Some(head_sha) {
            return Ok(Merging::HeadChanged);
        }
        pull_request.state = PullRequestState::Merged;
        Ok(Merging::Merged)
    }
}
