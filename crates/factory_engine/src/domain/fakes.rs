//! In-memory stand-ins for the factory's records, for the domain's tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use agent_session::domain::session_task::TaskDocumentId;
use chrono::Utc;
use factory_records::domain::models::{
    ClaimedSession, EntryId, Gate, HandBack, Outcome, Run, RunEnd, RunId, RunInsert, RunOutcome,
    SessionStep, Step, StepEnd, StepKey, StepSession, StoppingSession, TaskLink,
};
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::ports::{Retryable, TaskAccess, TaskDirectory, UserDirectory};

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
}

/// Everyone has an account but `macro|nobody@example.com`.
impl UserDirectory for FakeRepo {
    type Err = FakeError;

    async fn user_exists(&self, user_id: &MacroUserIdStr<'static>) -> Result<bool, FakeError> {
        Ok(user_id.as_ref() != "macro|nobody@example.com")
    }
}

/// Every task is visible to everyone.
impl TaskDirectory for FakeRepo {
    type Err = FakeError;

    async fn task_access(
        &self,
        _user_id: &MacroUserIdStr<'static>,
        task: &TaskDocumentId,
    ) -> Result<TaskAccess, FakeError> {
        let id = Uuid::parse_str(task.as_str()).map_err(|_| FakeError)?;
        if self.tasks.lock().expect("tasks").contains(&id) {
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
        request: &str,
        user_id: &MacroUserIdStr<'static>,
        task_id: Option<Uuid>,
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

    async fn record_prepare(&self, key: StepKey) -> Result<bool, FakeError> {
        let run_ended = self
            .runs
            .lock()
            .expect("runs")
            .get(&key.run_id)
            .is_none_or(|run| run.end.is_some());
        if run_ended || self.step_ended(key) {
            return Ok(false);
        }
        let mut sessions = self.sessions.lock().expect("sessions");
        let claimed = sessions
            .iter_mut()
            .find(|claimed| claimed.key == key && !claimed.released);
        let recorded = claimed.is_some();
        if let Some(claimed) = claimed {
            claimed.prepared_at = Some(Utc::now());
        }
        Ok(recorded)
    }

    async fn forget_prepare(&self, key: StepKey) -> Result<(), FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions
            .iter_mut()
            .find(|claimed| claimed.key == key && !claimed.opened)
        {
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
