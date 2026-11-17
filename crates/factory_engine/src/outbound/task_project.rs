//! The project a task is in, as its Project property names it: an
//! initiative. A task the factory files beside a Run's task (a Main Hold's
//! task, a review's follow-up) goes in the Run task's project, read and set
//! here the same way for each.

#[cfg(test)]
pub(crate) mod fixture;

use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::task::{
    NewTaskProperty, NewTaskPropertyValue, TaskEntityReference,
};
use macro_user_id::user_id::MacroUserIdStr;
use models_properties::EntityType;
use models_properties::service::property_value::PropertyValue;
use system_properties::SystemPropertyKey;
use uuid::Uuid;

/// The initiative the task document `task_id` is in, as its Project
/// property names it and `owner` sees it; none when it is in none, or the
/// owner no longer sees the task. An error when document storage did not
/// answer, or answered with something that does not read.
pub(crate) async fn initiative_of(
    documents: &DocumentStorageServiceClient,
    owner: &MacroUserIdStr<'static>,
    task_id: Uuid,
) -> Result<Option<String>, rootcause::Report> {
    let properties = documents
        .get_document_properties(&task_id.to_string(), owner.as_ref())
        .await
        .map_err(|error| rootcause::report!("{error:#}"))?;
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

/// The Project property that puts a new task in `initiative`.
pub(crate) fn project_property(initiative: String) -> NewTaskProperty {
    NewTaskProperty {
        property_id: SystemPropertyKey::PROJECT_UUID.to_string(),
        value: NewTaskPropertyValue::EntityReference {
            reference: TaskEntityReference {
                entity_id: initiative,
                entity_type: EntityType::Initiative,
            },
        },
    }
}
