//! The project a task is in, as its Project property names it: an
//! initiative. A task the factory files beside a Run's task (a Main Hold's
//! task, a review's follow-up) goes in the Run task's project, read here,
//! and starts with the properties set here the same way for each.

#[cfg(test)]
pub(crate) mod fixture;

use document_storage_service_client::DocumentStorageServiceClient;
use document_storage_service_client::task::{
    NewTaskProperty, NewTaskPropertyValue, PropertiesReadError, TaskEntityReference,
};
use macro_user_id::user_id::MacroUserIdStr;
use models_properties::service::property_value::PropertyValue;
use models_properties::{DataType, EntityType};
use system_properties::{StatusOption, SystemPropertyKey};
use uuid::Uuid;

/// The initiative the task document `task_id` is in, as its Project
/// property names it and `owner` sees it; none when it is in none, the
/// owner no longer sees the task, or its properties do not read (with a
/// warning: reading them again would read the same). An error when
/// document storage did not answer, which is worth trying again.
pub(crate) async fn initiative_of(
    documents: &DocumentStorageServiceClient,
    owner: &MacroUserIdStr<'static>,
    task_id: Uuid,
) -> Result<Option<String>, rootcause::Report> {
    let properties = match documents
        .get_document_properties(&task_id.to_string(), owner.as_ref())
        .await
    {
        Ok(Some(properties)) => properties,
        Ok(None) => return Ok(None),
        Err(PropertiesReadError::Unreadable(error)) => {
            tracing::warn!(
                %error,
                %task_id,
                "the run task's properties do not read: its project is taken as none"
            );
            return Ok(None);
        }
        Err(PropertiesReadError::Unavailable(error)) => return Err(error),
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

/// The tags the task document `task_id` carries, as `owner` sees them (their
/// own tags and their team's), by label, in the order its tag properties
/// list them; none when the owner no longer sees the task, or its
/// properties do not read (with a warning: reading them again would read
/// the same). An error when document storage did not answer, which is
/// worth trying again.
pub(crate) async fn tags_of(
    documents: &DocumentStorageServiceClient,
    owner: &MacroUserIdStr<'static>,
    task_id: Uuid,
) -> Result<Vec<String>, rootcause::Report> {
    let properties = match documents
        .get_document_properties(&task_id.to_string(), owner.as_ref())
        .await
    {
        Ok(Some(properties)) => properties,
        Ok(None) => return Ok(Vec::new()),
        Err(PropertiesReadError::Unreadable(error)) => {
            tracing::warn!(
                %error,
                %task_id,
                "the run task's properties do not read: it is taken to carry no tags"
            );
            return Ok(Vec::new());
        }
        Err(PropertiesReadError::Unavailable(error)) => return Err(error),
    };
    let mut tags = Vec::new();
    for property in properties.properties {
        if property.definition.data_type != DataType::Tag {
            continue;
        }
        let Some(PropertyValue::SelectOption(chosen)) = property.value else {
            continue;
        };
        let options = property.options.unwrap_or_default();
        for id in chosen {
            let label = options
                .iter()
                .find(|option| option.id == id)
                .and_then(|option| option.value.as_string());
            if let Some(label) = label {
                tags.push(label.to_string());
            }
        }
    }
    Ok(tags)
}

/// The properties a task the factory files for `owner` starts with: the
/// owner as its assignee and Not started as its status, the defaults
/// document storage gives a task made with none, and its Project, when it
/// goes in the initiative `initiative`. Naming any property drops those
/// defaults, so they are named too, as the app names them.
pub(crate) fn new_task_properties(
    owner: &MacroUserIdStr<'static>,
    initiative: Option<String>,
) -> Vec<NewTaskProperty> {
    let assignee = NewTaskProperty {
        property_id: SystemPropertyKey::ASSIGNEES_UUID.to_string(),
        value: NewTaskPropertyValue::MultiEntityReference {
            references: vec![TaskEntityReference {
                entity_id: owner.as_ref().to_string(),
                entity_type: EntityType::User,
            }],
        },
    };
    let status = NewTaskProperty {
        property_id: SystemPropertyKey::STATUS_UUID.to_string(),
        value: NewTaskPropertyValue::SelectOption {
            option_id: StatusOption::NOT_STARTED_UUID,
        },
    };
    let project = initiative.map(|initiative| NewTaskProperty {
        property_id: SystemPropertyKey::PROJECT_UUID.to_string(),
        value: NewTaskPropertyValue::EntityReference {
            reference: TaskEntityReference {
                entity_id: initiative,
                entity_type: EntityType::Initiative,
            },
        },
    });
    [assignee, status].into_iter().chain(project).collect()
}
