//! [`HoldTasks`] through the document storage service, as the Run's owner.

#[cfg(test)]
mod test;

use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::task::{NewTask, NewTaskProperty};
use macro_user_id::user_id::MacroUserIdStr;
use models_properties::api::requests::SetPropertyValue;
use models_properties::api::responses::PropertyValue;
use models_properties::{EntityReference, EntityType};
use system_properties::SystemPropertyKey;
use uuid::Uuid;

use crate::domain::main_hold::HoldTask;
use crate::domain::ports::HoldTasks;

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

    /// The initiative of the task document `task_id`, as `owner` sees it;
    /// none when it has none, or the owner no longer sees it.
    async fn initiative_of(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Result<Option<String>, DocumentHoldTasksErr> {
        let properties = self
            .documents
            .get_document_properties(&task_id.to_string(), owner.as_ref())
            .await
            .map_err(|error| DocumentHoldTasksErr(rootcause::report!("{error:#}")))?;
        let Some(properties) = properties else {
            return Ok(None);
        };
        Ok(properties
            .properties
            .into_iter()
            .filter(|property| property.definition.id == SystemPropertyKey::PROJECT_UUID)
            .find_map(|property| match property.value {
                Some(PropertyValue::EntityRef(references)) => references
                    .into_iter()
                    .find(|reference| reference.entity_type == EntityType::Initiative)
                    .map(|reference| reference.entity_id),
                _ => None,
            }))
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
    ) -> Result<Uuid, DocumentHoldTasksErr> {
        let initiative = match beside {
            Some(task_id) => self.initiative_of(owner, task_id).await?,
            None => None,
        };
        let property_values = initiative
            .map(|initiative| NewTaskProperty {
                property_id: SystemPropertyKey::PROJECT_UUID.to_string(),
                value: SetPropertyValue::EntityReference {
                    reference: EntityReference::new(initiative, EntityType::Initiative),
                },
            })
            .into_iter()
            .collect();
        let created = self
            .documents
            .create_task(
                &NewTask {
                    task_name: task.title.clone(),
                    markdown: task.markdown.clone(),
                    property_values,
                },
                owner.as_ref(),
            )
            .await
            .map_err(|error| DocumentHoldTasksErr(rootcause::report!("{error:#}")))?;
        Uuid::parse_str(&created).map_err(|_| {
            DocumentHoldTasksErr(rootcause::report!("the new task's id {created} is no UUID"))
        })
    }
}
