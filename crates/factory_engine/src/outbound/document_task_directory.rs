//! [`TaskDirectory`] over Macro's entity access checks and the document
//! storage service.

use agent_session::domain::session_task::TaskDocumentId;
use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::task::{CreateTaskError, NewTask};
use document_sub_type::DocumentSubType;
use entity_access::domain::models::{AccessError, AccessLevel, EntityType, ViewAccessLevel};
use entity_access::domain::ports::EntityAccessService;
use macro_user_id::user_id::MacroUserIdStr;

use uuid::Uuid;

#[cfg(test)]
mod test;

use crate::domain::ports::{FollowUpMade, FollowUpTask, TaskAccess, TaskDirectory};

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
        task_id: Uuid,
    ) -> Result<FollowUpMade, Self::Err> {
        let access = &self.access;
        self.make_follow_up(user_id, of, follow_up, task_id, async |project| {
            may_add_to(access, user_id, project).await
        })
        .await
    }
}

/// Whether `user_id` may add a task to `project`, as document storage's
/// project check decides: at least edit access. A project they cannot
/// reach, or one that is gone, is a no; only a failed check is an error.
async fn may_add_to(
    access: &impl EntityAccessService,
    user_id: &MacroUserIdStr<'static>,
    project: Uuid,
) -> Result<bool, DocumentTaskDirectoryErr> {
    let level = access
        .get_access_level(Some(user_id), &project.to_string(), EntityType::Project)
        .await;
    match level {
        Ok(level) => Ok(matches!(
            level,
            Some(AccessLevel::Edit | AccessLevel::Owner)
        )),
        Err(
            AccessError::Unauthorized
            | AccessError::UnauthorizedWithMessage(_)
            | AccessError::NotFound(_)
            | AccessError::BadRequest(_),
        ) => Ok(false),
        Err(error) => Err(DocumentTaskDirectoryErr::Access(error)),
    }
}

impl<A> DocumentTaskDirectory<A> {
    /// [`TaskDirectory::create_follow_up`], with `may_add` saying whether the
    /// owner may add a task to the project it goes in. One they may not add
    /// to is refused before it is asked for: document storage answers that
    /// with the same 401 as a key it does not accept, which is worth trying
    /// again.
    async fn make_follow_up(
        &self,
        user_id: &MacroUserIdStr<'static>,
        of: Option<Uuid>,
        follow_up: &FollowUpTask,
        task_id: Uuid,
        may_add: impl AsyncFnOnce(Uuid) -> Result<bool, DocumentTaskDirectoryErr>,
    ) -> Result<FollowUpMade, DocumentTaskDirectoryErr> {
        // An earlier attempt may have made it: its id says so.
        if self.made(task_id).await? {
            return Ok(FollowUpMade::Made);
        }

        // A follow-up goes in the project of the task it follows up; the
        // owner must still be able to add to it, as when they make a task.
        // It is the owner's own, unshared, until they share it.
        let project_id = match of {
            Some(of) => self
                .documents
                .get_document_basic(&of.to_string())
                .await
                .map_err(documents)?
                .and_then(|document| document.project_id)
                .and_then(|project| Uuid::parse_str(&project).ok()),
            None => None,
        };
        if let Some(project) = project_id
            && !may_add(project).await?
        {
            return Ok(FollowUpMade::Refused);
        }
        let task = NewTask {
            task_name: follow_up.name.clone(),
            markdown: follow_up.markdown.clone(),
            project_id,
            share_with_team: false,
            document_id: Some(task_id),
            property_values: Vec::new(),
        };
        match self
            .documents
            .create_task_for_user(&task, user_id.as_ref())
            .await
        {
            // A task made under another id would be made again on every
            // retry: that is a fault to stop at, never to try again.
            Ok(made) if made == task_id.to_string() => Ok(FollowUpMade::Made),
            Ok(made) => Ok(FollowUpMade::OtherId(made)),
            // A refusal may be the conflict of an attempt that made it
            // meanwhile.
            Err(CreateTaskError::Refused(_)) if self.made(task_id).await? => Ok(FollowUpMade::Made),
            Err(CreateTaskError::Refused(_)) => Ok(FollowUpMade::Refused),
            Err(error) => Err(error.into()),
        }
    }

    /// Whether the task `task_id` is there.
    async fn made(&self, task_id: Uuid) -> Result<bool, DocumentTaskDirectoryErr> {
        let document = self
            .documents
            .get_document_basic(&task_id.to_string())
            .await
            .map_err(documents)?;
        Ok(document.is_some_and(|document| document.deleted_at.is_none()))
    }
}

fn documents(error: impl std::fmt::Display) -> DocumentTaskDirectoryErr {
    DocumentTaskDirectoryErr::Documents(rootcause::report!("{error:#}"))
}
