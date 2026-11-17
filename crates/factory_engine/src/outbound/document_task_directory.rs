//! [`TaskDirectory`] over Macro's entity access checks and the document
//! storage service.

use agent_session::domain::session_task::TaskDocumentId;
use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::task::{CreateTaskError, NewTask};
use document_sub_type::DocumentSubType;
use entity_access::domain::models::{AccessError, EntityType, ViewAccessLevel};
use entity_access::domain::ports::EntityAccessService;
use macro_user_id::user_id::MacroUserIdStr;

use uuid::Uuid;

use crate::domain::ports::{FollowUpTask, TaskAccess, TaskDirectory};

/// Whether a person may see a document comes from entity access, and
/// whether it is a task from the document storage service, which owns
/// documents.
#[derive(Clone)]
pub struct DocumentTaskDirectory<A> {
    access: A,
    documents: DocumentStorageServiceClient,
}

impl<A: EntityAccessService> DocumentTaskDirectory<A> {
    /// The directory, over entity access and the document storage service.
    pub fn new(access: A, documents: DocumentStorageServiceClient) -> Self {
        Self { access, documents }
    }
}

/// Errors reading a task.
#[derive(Debug, thiserror::Error)]
pub enum DocumentTaskDirectoryErr {
    /// Access could not be checked.
    #[error("the access check failed: {0}")]
    Access(AccessError),
    /// The document storage service did not answer.
    #[error("the document could not be read: {0:?}")]
    Documents(rootcause::Report),
    /// The task could not be made.
    #[error("the task could not be made: {0}")]
    Create(#[from] CreateTaskError),
    /// The task made has an id that is not a UUID.
    #[error("the task made has id {0:?}, which is not a UUID")]
    TaskId(String),
}

impl<A: EntityAccessService> TaskDirectory for DocumentTaskDirectory<A> {
    type Err = DocumentTaskDirectoryErr;

    #[tracing::instrument(err, skip(self, user_id), fields(task = task.as_str()))]
    async fn task_access(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task: &TaskDocumentId,
    ) -> Result<TaskAccess, Self::Err> {
        let document_id = task.as_str();

        // A hidden document and a missing one are the same answer.
        let receipt = self
            .access
            .generate_entity_access_receipt::<ViewAccessLevel>(
                user_id,
                None,
                document_id,
                EntityType::Document,
            )
            .await;
        match receipt {
            Ok(_) => {}
            Err(
                AccessError::Unauthorized
                | AccessError::UnauthorizedWithMessage(_)
                | AccessError::NotFound(_)
                | AccessError::BadRequest(_),
            ) => return Ok(TaskAccess::NotFound),
            Err(error) => return Err(DocumentTaskDirectoryErr::Access(error)),
        }

        let document = self
            .documents
            .get_document_basic(document_id)
            .await
            .map_err(|error| {
                DocumentTaskDirectoryErr::Documents(rootcause::report!("{error:#}"))
            })?;
        let Some(document) = document.filter(|document| document.deleted_at.is_none()) else {
            return Ok(TaskAccess::NotFound);
        };
        if document.sub_type == Some(DocumentSubType::Task) {
            Ok(TaskAccess::Visible)
        } else {
            Ok(TaskAccess::NotATask)
        }
    }

    #[tracing::instrument(err, skip(self, user_id, follow_up))]
    async fn create_follow_up(
        &self,
        user_id: &MacroUserIdStr<'static>,
        of: Option<Uuid>,
        follow_up: &FollowUpTask,
    ) -> Result<Uuid, Self::Err> {
        // A follow-up goes in the project of the task it follows up; the
        // owner must still be able to add to it, as when they make a task.
        let project_id = match of {
            Some(of) => self
                .documents
                .get_document_basic(&of.to_string())
                .await
                .map_err(|error| {
                    DocumentTaskDirectoryErr::Documents(rootcause::report!("{error:#}"))
                })?
                .and_then(|document| document.project_id)
                .and_then(|project| Uuid::parse_str(&project).ok()),
            None => None,
        };
        let task = NewTask {
            task_name: follow_up.name.clone(),
            markdown: follow_up.markdown.clone(),
            project_id,
            share_with_team: true,
        };
        let made = self
            .documents
            .create_task_for_user(&task, user_id.as_ref())
            .await?;
        Uuid::parse_str(&made).map_err(|_| DocumentTaskDirectoryErr::TaskId(made))
    }
}
