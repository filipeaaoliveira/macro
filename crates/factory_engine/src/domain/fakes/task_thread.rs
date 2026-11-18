//! A stand-in for the task thread: comments kept in memory, as document
//! storage keeps them, each once by its id.

use std::sync::{Arc, Mutex};

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::FakeError;
use crate::domain::ports::{TaskThread, ThreadPosted};
use crate::domain::task_thread::ThreadComment;

/// The comments posted, on which task, in order: a comment posted again
/// under its id is kept once, and a reply to a thread not posted is not
/// there. Fails every post when `down` is set.
#[derive(Clone, Default)]
pub(crate) struct FakeTaskThread {
    pub(crate) posted: Arc<Mutex<Vec<(Uuid, ThreadComment)>>>,
    pub(crate) down: bool,
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
}
