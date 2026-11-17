//! Tasks and their properties, read and created as an acting user: a
//! service makes a task as a user, in a project, as `POST
//! /documents/create_task` makes them for the user themselves.

#[cfg(test)]
mod test;

use anyhow::Result;
use models_properties::EntityType;
use models_properties::api::responses::EntityPropertiesResponse;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::DocumentStorageServiceClient;
use crate::constants::{
    INTERNAL_API_KEY_HEADER, INTERNAL_MACRO_USER_ID_HEADER, MACRO_INTERNAL_USER_ID_HEADER_KEY,
};

/// A task to make: its name, its markdown, the project it goes in, the
/// properties it starts with, and the id to make it with.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    /// The task's name.
    pub task_name: String,
    /// Its text, as markdown.
    pub markdown: String,
    /// The project it goes in; the user's own tasks when none.
    pub project_id: Option<uuid::Uuid>,
    /// Whether it is shared with the user's team, as tasks people make are.
    pub share_with_team: bool,
    /// The properties it starts with; none by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_values: Vec<NewTaskProperty>,
    /// The id to make it with, so a retry finds the task rather than
    /// making a second one; one of document storage's when none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_id: Option<uuid::Uuid>,
}

impl NewTask {
    /// A task named `task_name` with `markdown`, shared with the user's
    /// team, in no project, with no properties and an id document storage
    /// picks.
    pub fn new(task_name: impl Into<String>, markdown: impl Into<String>) -> Self {
        Self {
            task_name: task_name.into(),
            markdown: markdown.into(),
            project_id: None,
            share_with_team: true,
            property_values: Vec::new(),
            document_id: None,
        }
    }
}

/// A property a new task starts with: the subset of the properties API's
/// `PropertyInput` this client sets, as `POST /documents/create_task`
/// reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTaskProperty {
    /// The property definition's id.
    pub property_id: String,
    /// Its value.
    pub value: NewTaskPropertyValue,
}

/// A new task's property value, as the properties API's `SetPropertyValue`
/// writes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum NewTaskPropertyValue {
    /// A reference to one entity, such as the initiative a task is in.
    EntityReference {
        /// The entity.
        reference: TaskEntityReference,
    },
}

/// An entity a new task's property refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskEntityReference {
    /// The entity's id.
    pub entity_id: String,
    /// Its type.
    pub entity_type: EntityType,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedTask {
    document_id: String,
}

/// Why a task could not be made.
#[derive(thiserror::Error, Debug)]
pub enum CreateTaskError {
    /// Document storage refused it, for this status: the request does not
    /// read, or conflicts with a document there.
    #[error("the task was refused: HTTP {0}")]
    Refused(StatusCode),
    /// The request failed, or document storage failed, throttled, timed
    /// out, refused the key or the user (which a configuration or access
    /// change may fix), or answered with something unusable: worth trying
    /// again.
    #[error("{0}")]
    Unavailable(rootcause::Report),
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

    /// Make `task` as `user_id`, who owns it, and return its document's
    /// id. A task made with a `document_id` that already exists is
    /// refused, as a conflict.
    #[tracing::instrument(skip(self, task), err)]
    pub async fn create_task_for_user(
        &self,
        task: &NewTask,
        user_id: &str,
    ) -> Result<String, CreateTaskError> {
        let unavailable = |error: reqwest::Error| {
            CreateTaskError::Unavailable(rootcause::Report::new(error).into_dynamic())
        };
        let res = self
            .external_client
            .post(format!("{}/documents/create_task", self.url))
            .json(task)
            .header(INTERNAL_API_KEY_HEADER, &self.internal_auth_key)
            .header(INTERNAL_MACRO_USER_ID_HEADER, user_id)
            .send()
            .await
            .map_err(unavailable)?;

        let status = res.status();
        if refused(status) {
            let body = res.text().await.unwrap_or_default();
            tracing::warn!(%status, %body, "document storage refused a task");
            return Err(CreateTaskError::Refused(status));
        }
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(CreateTaskError::Unavailable(
                rootcause::report!("HTTP {status}: {body}").into_dynamic(),
            ));
        }

        let created: CreatedTask = res.json().await.map_err(unavailable)?;
        Ok(created.document_id)
    }

    /// Comment `content` on the passage `quote` of the document
    /// `document_id`, as `user_id`.
    #[tracing::instrument(skip(self, content), err)]
    pub async fn comment_on_text(
        &self,
        document_id: &str,
        user_id: &str,
        quote: &str,
        content: &str,
    ) -> Result<()> {
        self.client
            .post(format!(
                "{}/documents/{document_id}/comments/text",
                self.url
            ))
            .header(MACRO_INTERNAL_USER_ID_HEADER_KEY, user_id)
            .json(&serde_json::json!({ "content": content, "quote": quote }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

/// Whether `status` refuses the task for good. An unaccepted key or user
/// (401, 403), a timeout (408) and throttling (429) may pass, so they are
/// tried again; any other client error is the request's own.
fn refused(status: StatusCode) -> bool {
    let passing = [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::TOO_MANY_REQUESTS,
    ];
    status.is_client_error() && !passing.contains(&status)
}
