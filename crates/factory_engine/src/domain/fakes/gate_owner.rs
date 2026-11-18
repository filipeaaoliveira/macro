//! Stand-ins for the owner section's ports: a notifier that keeps what it
//! was sent, and a task's comments kept in memory.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::FakeError;
use crate::domain::gate_owner::{GateNotice, OwnerNote};
use crate::domain::ports::{GateNotifier, TaskNotes};

/// Keeps every notice it was sent, in order.
#[derive(Clone, Default)]
pub(crate) struct FakeNotifier {
    pub(crate) sent: Arc<Mutex<Vec<GateNotice>>>,
}

impl GateNotifier for FakeNotifier {
    async fn notify(&self, notice: GateNotice) {
        self.sent.lock().expect("sent").push(notice);
    }
}

/// When the comments on a task were asked for: the task, and since when.
type Asked = (Uuid, DateTime<Utc>);

/// The owner's comments on tasks, and each time they were asked for: the
/// task and since when. Fails every read when `down` is set.
#[derive(Clone, Default)]
pub(crate) struct FakeTaskNotes {
    pub(crate) notes: Arc<Mutex<Vec<(Uuid, OwnerNote)>>>,
    pub(crate) asked: Arc<Mutex<Vec<Asked>>>,
    pub(crate) down: bool,
}

impl TaskNotes for FakeTaskNotes {
    type Err = FakeError;

    async fn owner_notes(
        &self,
        task_id: Uuid,
        _owner: &MacroUserIdStr<'static>,
        since: DateTime<Utc>,
    ) -> Result<Vec<OwnerNote>, FakeError> {
        self.asked.lock().expect("asked").push((task_id, since));
        if self.down {
            return Err(FakeError);
        }
        Ok(self
            .notes
            .lock()
            .expect("notes")
            .iter()
            .filter(|(task, note)| *task == task_id && note.written_at >= since)
            .map(|(_, note)| note.clone())
            .collect())
    }
}
