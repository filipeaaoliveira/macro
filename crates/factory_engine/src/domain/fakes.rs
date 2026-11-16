//! In-memory stand-ins for the factory's records, for the domain's tests.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use factory_records::domain::models::{
    ClaimedSession, EntryId, HandBack, Outcome, Run, RunEnd, RunId, RunOutcome, SessionStep, Step,
    StepEnd, StepSession, TaskLink,
};
use factory_records::domain::ports::{RunRepository, UserDirectory};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::ports::{TaskAccess, TaskDirectory};

/// The session an agent Step claimed, and what it handed back.
#[derive(Clone)]
pub(crate) struct FakeSession {
    pub(crate) run_id: RunId,
    pub(crate) entry: EntryId,
    pub(crate) visit: u16,
    pub(crate) session: StepSession,
    pub(crate) offered: Vec<Outcome>,
    pub(crate) handed: Option<HandBack>,
}

#[derive(Debug, thiserror::Error)]
#[error("fake failure")]
pub(crate) struct FakeError;

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
        task_id: Uuid,
    ) -> Result<TaskAccess, FakeError> {
        if self.tasks.lock().expect("tasks").contains(&task_id) {
            Ok(TaskAccess::Visible)
        } else if self.documents.lock().expect("documents").contains(&task_id) {
            Ok(TaskAccess::NotATask)
        } else {
            Ok(TaskAccess::NotFound)
        }
    }
}

impl RunRepository for FakeRepo {
    type Err = FakeError;

    async fn insert_run(
        &self,
        id: RunId,
        request: &str,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<Run, FakeError> {
        let run = Run {
            id,
            request: request.to_string(),
            user_id: user_id.clone(),
            created_at: Utc::now(),
            task_id: None,
            end: None,
        };
        self.runs.lock().expect("runs").insert(id, run.clone());
        Ok(run)
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

    async fn claim_session(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
        session: StepSession,
        offered: &[Outcome],
    ) -> Result<ClaimedSession, FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        let claimed = sessions.iter().find(|claimed| {
            claimed.run_id == id && claimed.entry == *entry && claimed.visit == visit
        });
        if let Some(claimed) = claimed {
            return Ok(ClaimedSession {
                session: claimed.session,
                new: false,
            });
        }
        sessions.push(FakeSession {
            run_id: id,
            entry: entry.clone(),
            visit,
            session,
            offered: offered.to_vec(),
            handed: None,
        });
        Ok(ClaimedSession { session, new: true })
    }

    async fn session_step(&self, session_id: Uuid) -> Result<Option<SessionStep>, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        let Some(claimed) = sessions
            .iter()
            .find(|claimed| claimed.session.session_id == session_id)
        else {
            return Ok(None);
        };
        let owner = self
            .runs
            .lock()
            .expect("runs")
            .get(&claimed.run_id)
            .map(|run| run.user_id.clone())
            .ok_or(FakeError)?;
        let ended = self.steps.lock().expect("steps").iter().any(|(run, step)| {
            *run == claimed.run_id
                && step.entry == claimed.entry
                && step.visit == claimed.visit
                && step.end.is_some()
        });
        Ok(Some(SessionStep {
            run_id: claimed.run_id,
            entry: claimed.entry.clone(),
            visit: claimed.visit,
            owner,
            offered: claimed.offered.clone(),
            ended,
        }))
    }

    async fn hand_back(&self, session_id: Uuid, hand_back: &HandBack) -> Result<(), FakeError> {
        let mut sessions = self.sessions.lock().expect("sessions");
        if let Some(claimed) = sessions
            .iter_mut()
            .find(|claimed| claimed.session.session_id == session_id)
        {
            claimed.handed = Some(hand_back.clone());
        }
        Ok(())
    }

    async fn handed_back(
        &self,
        id: RunId,
        entry: &EntryId,
        visit: u16,
    ) -> Result<Option<HandBack>, FakeError> {
        let sessions = self.sessions.lock().expect("sessions");
        Ok(sessions
            .iter()
            .find(|claimed| {
                claimed.run_id == id && claimed.entry == *entry && claimed.visit == visit
            })
            .and_then(|claimed| claimed.handed.clone()))
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
