//! [`TaskThread`] through the document storage service: a Run's updates
//! posted as comments on its task, as the Run's owner, through document
//! storage's `/messages/document/{id}` routes, each under the id the
//! update gives it and notifying no one; and the Run's thread found again
//! among the task's discussions by the link its first comment carries.

#[cfg(test)]
mod test;

use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::messages::{
    CommentPosted, NewDocumentComment, PostCommentError, ReadMessagesError,
};
use factory_records::domain::models::RunId;
use macro_user_id::user_id::MacroUserIdStr;
use messages::domain::ports::{MessageCursor, MessageTimelineQuery};
use uuid::Uuid;

use crate::domain::gate_owner::is_factory_comment;
use crate::domain::ports::{TaskThread, ThreadPosted};
use crate::domain::task_thread::ThreadComment;

/// The discussions read per page while looking for a Run's thread.
const PAGE_SIZE: u16 = 100;

/// The most pages of discussions read while looking for a Run's thread.
const MAX_PAGES: usize = 5;

/// Posts a Run's updates on its task.
#[derive(Clone)]
pub struct DocumentTaskThread {
    documents: DocumentStorageServiceClient,
    app_url: String,
}

impl DocumentTaskThread {
    /// The task thread, through the document storage service, linking Runs
    /// in the Macro app at `app_url`.
    pub fn new(documents: DocumentStorageServiceClient, app_url: &str) -> Self {
        Self {
            documents,
            app_url: app_url.trim_end_matches('/').to_string(),
        }
    }
}

/// Why a comment could not be posted, or a thread found.
#[derive(Debug, thiserror::Error)]
pub enum TaskThreadError {
    /// The comment was not posted.
    #[error(transparent)]
    Post(#[from] PostCommentError),
    /// The task's discussions did not read.
    #[error(transparent)]
    Read(#[from] ReadMessagesError),
}

impl TaskThread for DocumentTaskThread {
    type Err = TaskThreadError;

    /// A comment posted before under its id is posted; a task, or a
    /// thread, the owner no longer reaches is not there.
    async fn post(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        comment: &ThreadComment,
    ) -> Result<ThreadPosted, TaskThreadError> {
        let posted = self
            .documents
            .post_document_comment(
                task_id,
                owner.as_ref(),
                &NewDocumentComment {
                    id: comment.id,
                    content: &comment.text,
                    thread_id: comment.thread_id,
                    silent: true,
                },
            )
            .await?;
        Ok(match posted {
            CommentPosted::Posted | CommentPosted::AlreadyPosted => ThreadPosted::Posted,
            CommentPosted::NotThere => ThreadPosted::NoThread,
        })
    }

    /// The task's discussions are read newest first, a page at a time, as
    /// the owner; the first one the owner started with a comment of the
    /// factory's that names `marker` is the Run's. A task the owner no
    /// longer reads has none.
    async fn find_thread(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        marker: &str,
    ) -> Result<Option<Uuid>, TaskThreadError> {
        let mut cursor: Option<MessageCursor> = None;
        for _ in 0..MAX_PAGES {
            let query = MessageTimelineQuery {
                cursor: cursor.take(),
                limit: Some(PAGE_SIZE),
                ..MessageTimelineQuery::default()
            };
            let page = match self
                .documents
                .document_messages(task_id, owner.as_ref(), &query)
                .await
            {
                Ok(page) => page,
                Err(ReadMessagesError::NotThere(_)) => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let found = page.items.iter().map(|item| &item.message).find(|message| {
                message.deleted_at.is_none()
                    && message.sender_id.as_user() == Some(owner)
                    && is_factory_comment(&message.content)
                    && message.content.contains(marker)
            });
            if let Some(message) = found {
                return Ok(Some(message.id));
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(None)
    }

    fn run_link(&self, run_id: RunId) -> String {
        format!("{}/app/factory/runs/{run_id}", self.app_url)
    }
}
