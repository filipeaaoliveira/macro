//! [`HoldTasks`] through the document storage service, as the Run's owner.

#[cfg(test)]
mod test;

use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::task::{CreateTaskError, NewTask};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::gate_owner::factory_signed;
use crate::domain::main_hold::HoldTask;
use crate::domain::ports::HoldTasks;
use crate::outbound::task_project::{initiative_of, new_task_properties};

/// Files a hold's task as the Run's owner, in the initiative (a task's
/// Project property) of the Run's own task.
#[derive(Clone)]
pub struct DocumentHoldTasks {
    documents: DocumentStorageServiceClient,
}

impl DocumentHoldTasks {
    /// The tasks, through the document storage service.
    pub fn new(documents: DocumentStorageServiceClient) -> Self {
        Self { documents }
    }

    /// Whether a filing made the task document `task_id`, even one deleted
    /// since: a deleted hold task is not filed again.
    async fn made(&self, task_id: Uuid) -> Result<bool, DocumentHoldTasksErr> {
        let document = self
            .documents
            .get_document_basic(&task_id.to_string())
            .await
            .map_err(|error| DocumentHoldTasksErr(rootcause::report!("{error:#}")))?;
        Ok(document.is_some())
    }
}

/// The document storage service did not read or create the task.
#[derive(Debug, thiserror::Error)]
#[error("the task could not be filed: {0:?}")]
pub struct DocumentHoldTasksErr(rootcause::Report);

impl HoldTasks for DocumentHoldTasks {
    type Err = DocumentHoldTasksErr;

    #[tracing::instrument(err, skip(self, owner, task))]
    async fn file(
        &self,
        owner: &MacroUserIdStr<'static>,
        beside: Option<Uuid>,
        task: &HoldTask,
        task_id: Uuid,
    ) -> Result<Uuid, DocumentHoldTasksErr> {
        // An earlier filing may have made it: its id says so.
        if self.made(task_id).await? {
            return Ok(task_id);
        }
        let initiative = match beside {
            // A Run task whose properties do not read files the task in no
            // initiative; one document storage could not answer for is
            // tried again.
            Some(task_id) => initiative_of(&self.documents, owner, task_id)
                .await
                .map_err(DocumentHoldTasksErr)?,
            None => None,
        };
        let property_values = new_task_properties(owner, initiative);
        // Shared with the owner's team, as the tasks people make are.
        let created = self
            .documents
            .create_task_for_user(
                &NewTask {
                    property_values,
                    document_id: Some(task_id),
                    ..NewTask::new(task.title.clone(), task.markdown.clone())
                },
                owner.as_ref(),
            )
            .await;
        let created = match created {
            Ok(created) => created,
            // A refusal may be the conflict of a filing that made it
            // meanwhile.
            Err(CreateTaskError::Refused(_)) if self.made(task_id).await? => {
                return Ok(task_id);
            }
            Err(error) => return Err(DocumentHoldTasksErr(rootcause::report!("{error}"))),
        };
        Uuid::parse_str(&created).map_err(|_| {
            DocumentHoldTasksErr(rootcause::report!("the new task's id {created} is no UUID"))
        })
    }

    #[tracing::instrument(err, skip(self, owner, text))]
    async fn comment(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        quote: &str,
        text: &str,
    ) -> Result<(), DocumentHoldTasksErr> {
        self.documents
            .comment_on_text(
                &task_id.to_string(),
                owner.as_ref(),
                quote,
                &factory_signed(text),
            )
            .await
            .map_err(|error| DocumentHoldTasksErr(rootcause::report!("{error:#}")))
    }
}
