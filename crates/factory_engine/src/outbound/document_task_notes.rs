//! [`TaskNotes`] through the document storage service: the comment threads
//! on a Run's task, read as the Run's owner, of which their own comments
//! are their notes.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::messages::ReadMessagesError;
use macro_user_id::user_id::MacroUserIdStr;
use messages::domain::models::Message;
use messages::domain::ports::{MessageCursor, MessageTimelineQuery};
use uuid::Uuid;

use crate::domain::gate_owner::OwnerNote;
use crate::domain::ports::TaskNotes;

/// The discussions read per page, the most document storage returns.
const PAGE_SIZE: u16 = 100;

/// The most pages of discussions read for one Step: more than a person
/// comments on a task while a Run waits on them.
const MAX_PAGES: usize = 5;

/// Reads a Run's owner's comments on its task.
#[derive(Clone)]
pub struct DocumentTaskNotes {
    documents: DocumentStorageServiceClient,
}

impl DocumentTaskNotes {
    /// The comments, through the document storage service.
    pub fn new(documents: DocumentStorageServiceClient) -> Self {
        Self { documents }
    }
}

impl TaskNotes for DocumentTaskNotes {
    type Err = ReadMessagesError;

    /// The discussions with a comment at or after `since` are read newest
    /// first, a page at a time, and each one's replies from its preview,
    /// or from the whole thread when the preview holds only some. Of their
    /// live comments, those `owner` wrote at or after `since` are the
    /// notes, oldest first. A task the owner no longer reads has none.
    async fn owner_notes(
        &self,
        task_id: Uuid,
        owner: &MacroUserIdStr<'static>,
        since: DateTime<Utc>,
    ) -> Result<Vec<OwnerNote>, Self::Err> {
        let is_note = |message: &Message| {
            message.deleted_at.is_none()
                && message.created_at >= since
                && message.sender_id.as_user() == Some(owner)
        };
        let mut notes: Vec<OwnerNote> = Vec::new();
        let mut cursor: Option<MessageCursor> = None;
        for _ in 0..MAX_PAGES {
            let query = MessageTimelineQuery {
                cursor: cursor.take(),
                activity_after: Some(since),
                limit: Some(PAGE_SIZE),
                ..MessageTimelineQuery::default()
            };
            let page = match self
                .documents
                .document_messages(task_id, owner.as_ref(), &query)
                .await
            {
                Ok(page) => page,
                Err(ReadMessagesError::NotThere(status)) => {
                    tracing::warn!(%task_id, %status, "the run's task is not there for its owner");
                    return Ok(Vec::new());
                }
                Err(error) => return Err(error),
            };

            for item in page.items {
                let replies = if item.thread.reply_count > item.thread.preview.len() as i64 {
                    self.documents
                        .document_thread(task_id, owner.as_ref(), item.message.id)
                        .await?
                        .replies
                } else {
                    item.thread.preview
                };
                let comments = std::iter::once(item.message).chain(replies);
                notes.extend(comments.filter(is_note).map(|message| OwnerNote {
                    written_at: message.created_at,
                    text: message.content,
                }));
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        notes.sort_by_key(|note| note.written_at);
        Ok(notes)
    }
}
