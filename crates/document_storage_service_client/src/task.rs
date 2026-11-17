//! Tasks and their properties, read and created as an acting user.

#[cfg(test)]
mod test;

use anyhow::Result;
use models_properties::api::requests::SetPropertyValue;
use models_properties::api::responses::EntityPropertiesResponse;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::DocumentStorageServiceClient;
use crate::constants::MACRO_INTERNAL_USER_ID_HEADER_KEY;

/// A task to create: the body of `POST /documents/create_task`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    /// Its name.
    pub task_name: String,
    /// Its text, as markdown.
    pub markdown: String,
    /// The properties it starts with.
    pub property_values: Vec<NewTaskProperty>,
}

/// A property a new task starts with.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTaskProperty {
    /// The property definition's id.
    pub property_id: String,
    /// Its value.
    pub value: SetPropertyValue,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedTask {
    document_id: String,
}

impl DocumentStorageServiceClient {
    /// The properties of the document `document_id`, tasks included, as
    /// `user_id` sees them; none when it is missing or they cannot see it.
    #[tracing::instrument(skip(self), err)]
    pub async fn get_document_properties(
        &self,
        document_id: &str,
        user_id: &str,
    ) -> Result<Option<EntityPropertiesResponse>> {
        let response = self
            .client
            .get(format!(
                "{}/properties/entities/DOCUMENT/{document_id}",
                self.url
            ))
            .header(MACRO_INTERNAL_USER_ID_HEADER_KEY, user_id)
            .send()
            .await?;
        if matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::NOT_FOUND
        ) {
            return Ok(None);
        }
        Ok(Some(response.error_for_status()?.json().await?))
    }

    /// Create `task` as `user_id`, and return its document's id.
    #[tracing::instrument(skip(self, task), err)]
    pub async fn create_task(&self, task: &NewTask, user_id: &str) -> Result<String> {
        let response = self
            .client
            .post(format!("{}/documents/create_task", self.url))
            .header(MACRO_INTERNAL_USER_ID_HEADER_KEY, user_id)
            .json(task)
            .send()
            .await?
            .error_for_status()?;
        let created: CreatedTask = response.json().await?;
        Ok(created.document_id)
    }
}
