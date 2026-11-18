//! [`TaskThread`] through the document storage service: a Run's updates
//! posted as comments on its task, as the Run's owner, through document
//! storage's `/messages/document/{id}` route, each under the id the update
//! gives it.

#[cfg(test)]
mod test;

use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::messages::{
    CommentPosted, NewDocumentComment, PostCommentError,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::ports::{TaskThread, ThreadPosted};
use crate::domain::task_thread::ThreadComment;

/// Posts a Run's updates on its task.
#[derive(Clone)]
pub struct DocumentTaskThread {
    documents: DocumentStorageServiceClient,
}

impl DocumentTaskThread {
    /// The task thread, through the document storage service.
    pub fn new(documents: DocumentStorageServiceClient) -> Self {
        Self { documents }
    }
}

impl TaskThread for DocumentTaskThread {
    type Err = PostCommentError;

    /// A comment posted before under its id is posted; a task, or a
    /// thread, the owner no longer reaches is not there.
    async fn post(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        comment: &ThreadComment,
    ) -> Result<ThreadPosted, PostCommentError> {
        let posted = self
            .documents
            .post_document_comment(
                task_id,
                owner.as_ref(),
                &NewDocumentComment {
                    id: comment.id,
                    content: &comment.text,
                    thread_id: comment.thread_id,
                },
            )
            .await?;
        Ok(match posted {
            CommentPosted::Posted | CommentPosted::AlreadyPosted => ThreadPosted::Posted,
            CommentPosted::NotThere => ThreadPosted::NoThread,
        })
    }
}
