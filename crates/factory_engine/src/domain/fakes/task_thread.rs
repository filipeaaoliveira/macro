//! A stand-in for the task thread: comments kept in memory, as document
//! storage keeps them, each once by its id, and only under an id stamped
//! within a day of now, as Macro takes a comment's own id.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeDelta, Utc};
use factory_records::domain::models::RunId;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::FakeError;
use crate::domain::gate_owner::is_factory_comment;
use crate::domain::ports::{TaskThread, ThreadPosted};
use crate::domain::task_thread::ThreadComment;

/// How far from now a comment's own id may be stamped, as Macro allows.
const ID_WINDOW: TimeDelta = TimeDelta::days(1);

/// The comments posted, on which task, in order: a comment posted again
/// under its id is kept once, a reply to a thread not posted is not there,
/// and an id stamped more than a day from now is refused. Fails every post
/// when `down` is set.
#[derive(Clone, Default)]
pub(crate) struct FakeTaskThread {
    pub(crate) posted: Arc<Mutex<Vec<(Uuid, ThreadComment)>>>,
    pub(crate) down: bool,
    /// How many times a Run's thread was looked for.
    pub(crate) looked_for: Arc<Mutex<u16>>,
}

impl FakeTaskThread {
    /// The texts posted, in order.
    pub(crate) fn texts(&self) -> Vec<String> {
        self.posted
            .lock()
            .expect("posted")
            .iter()
            .map(|(_, comment)| comment.text.clone())
            .collect()
    }
}

/// When the UUIDv7 `id` was stamped.
fn stamped(id: Uuid) -> Option<DateTime<Utc>> {
    let (seconds, nanos) = id.get_timestamp()?.to_unix();
    DateTime::from_timestamp(i64::try_from(seconds).ok()?, nanos)
}

impl TaskThread for FakeTaskThread {
    type Err = FakeError;

    async fn post(
        &self,
        _owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        comment: &ThreadComment,
    ) -> Result<ThreadPosted, FakeError> {
        if self.down {
            return Err(FakeError);
        }
        let in_window = stamped(comment.id).is_some_and(|at| (Utc::now() - at).abs() <= ID_WINDOW);
        if comment.id.get_version_num() != 7 || !in_window {
            return Err(FakeError);
        }
        let mut posted = self.posted.lock().expect("posted");
        if posted.iter().any(|(_, kept)| kept.id == comment.id) {
            return Ok(ThreadPosted::Posted);
        }
        if let Some(root) = comment.thread_id
            && !posted.iter().any(|(_, kept)| kept.id == root)
        {
            return Ok(ThreadPosted::NoThread);
        }
        posted.push((task_id, comment.clone()));
        Ok(ThreadPosted::Posted)
    }

    async fn find_thread(
        &self,
        _owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        marker: &str,
    ) -> Result<Option<Uuid>, FakeError> {
        *self.looked_for.lock().expect("looked for") += 1;
        Ok(self
            .posted
            .lock()
            .expect("posted")
            .iter()
            .find(|(on, comment)| {
                *on == task_id
                    && comment.thread_id.is_none()
                    && is_factory_comment(&comment.text)
                    && comment.text.contains(marker)
            })
            .map(|(_, comment)| comment.id))
    }

    fn run_link(&self, run_id: RunId) -> String {
        format!("https://macro.test/app/factory/runs/{run_id}")
    }
}
