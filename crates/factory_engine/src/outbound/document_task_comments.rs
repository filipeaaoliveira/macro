//! The comments on a Run's task, read as its owner through the document
//! storage service's discussions: what the owner wrote themselves there.
//! A comment the factory or an agent posted for them, and one anchored to
//! a passage of the task, as the factory's own are, is not theirs to read
//! as a reply.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use document_storage_service_client::DocumentStorageServiceClient;
use macro_user_id::user_id::MacroUserIdStr;
use messages::domain::models::{Message, MessageListItem};
use uuid::Uuid;

use crate::domain::ports::{TaskComment, TaskComments};

/// Why the comments could not be read.
#[derive(Debug, thiserror::Error)]
#[error("the task's comments could not be read: {0}")]
pub struct TaskCommentsError(String);

/// The comments on Run tasks, through the document storage service.
#[derive(Clone)]
pub struct DocumentTaskComments {
    documents: DocumentStorageServiceClient,
}

impl DocumentTaskComments {
    /// The comments, read through `documents`.
    pub fn new(documents: DocumentStorageServiceClient) -> Self {
        Self { documents }
    }
}

impl TaskComments for DocumentTaskComments {
    type Err = TaskCommentsError;

    #[tracing::instrument(err, skip(self, owner))]
    async fn owner_comments_after(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        after: DateTime<Utc>,
    ) -> Result<Vec<TaskComment>, TaskCommentsError> {
        let page = self
            .documents
            .document_discussions_after(&task_id.to_string(), owner.as_ref(), after)
            .await
            .map_err(|error| TaskCommentsError(error.to_string()))?;
        Ok(owner_comments(&page.items, owner, after))
    }
}

/// What `owner` wrote themselves after `after` among the discussions
/// `items`: each thread's first comment and the replies its preview holds,
/// oldest first.
pub(crate) fn owner_comments(
    items: &[MessageListItem],
    owner: &MacroUserIdStr<'static>,
    after: DateTime<Utc>,
) -> Vec<TaskComment> {
    let mut comments: Vec<TaskComment> = items
        .iter()
        .flat_map(|item| std::iter::once(&item.message).chain(&item.thread.preview))
        .filter(|message| written_by(message, owner) && message.created_at > after)
        .map(|message| TaskComment {
            text: message.content.clone(),
            created_at: message.created_at,
        })
        .collect();
    comments.sort_by_key(|comment| comment.created_at);
    comments
}

/// Whether `owner` wrote `message` themselves: not deleted, and not posted
/// by a bot, nor by a program or an agent acting for them.
fn written_by(message: &Message, owner: &MacroUserIdStr<'static>) -> bool {
    message.deleted_at.is_none()
        && message.sender_id.as_user() == Some(owner)
        && message.triggered_by.is_none()
        && message.triggered_via.is_none()
        && message.bot_profile.is_none()
}
