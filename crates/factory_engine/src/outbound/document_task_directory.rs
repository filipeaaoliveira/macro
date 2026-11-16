//! [`TaskDirectory`] over Macro's entity access checks and the document
//! storage service.

use document_storage_service_client::DocumentStorageServiceClient;
use document_sub_type::DocumentSubType;
use entity_access::domain::models::{AccessError, EntityType, ViewAccessLevel};
use entity_access::domain::ports::EntityAccessService;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::ports::{TaskAccess, TaskDirectory};

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
    Documents(anyhow::Error),
}

impl<A: EntityAccessService> TaskDirectory for DocumentTaskDirectory<A> {
    type Err = DocumentTaskDirectoryErr;

    #[tracing::instrument(err, skip(self, user_id))]
    async fn task_access(
        &self,
        user_id: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Result<TaskAccess, Self::Err> {
        let document_id = task_id.to_string();

        // A hidden document and a missing one are the same answer.
        let receipt = self
            .access
            .generate_entity_access_receipt::<ViewAccessLevel>(
                user_id,
                None,
                &document_id,
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
            .get_document_basic(&document_id)
            .await
            .map_err(DocumentTaskDirectoryErr::Documents)?;
        let Some(document) = document.filter(|document| document.deleted_at.is_none()) else {
            return Ok(TaskAccess::NotFound);
        };
        if document.sub_type == Some(DocumentSubType::Task) {
            Ok(TaskAccess::Visible)
        } else {
            Ok(TaskAccess::NotATask)
        }
    }
}
