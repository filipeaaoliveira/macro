//! Tasks a service makes as a user, in a project, as `POST
//! /documents/create_task` makes them for the user themselves.

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use super::DocumentStorageServiceClient;
use crate::constants::{INTERNAL_API_KEY_HEADER, INTERNAL_MACRO_USER_ID_HEADER};

#[cfg(test)]
mod test;

/// A task to make: its name, its markdown, the project it goes in, and the
/// id to make it with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTask {
    /// The task's name.
    pub task_name: String,
    /// Its text, as markdown.
    pub markdown: String,
    /// The project it goes in; the user's own tasks when none.
    pub project_id: Option<uuid::Uuid>,
    /// Whether it is shared with the user's team.
    pub share_with_team: bool,
    /// The id to make it with, so a retry finds the task rather than
    /// making a second one; one of document storage's when none.
    pub document_id: Option<uuid::Uuid>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreatedTask {
    document_id: String,
}

/// Why a task could not be made.
#[derive(thiserror::Error, Debug)]
pub enum CreateTaskError {
    /// Document storage refused it, for this status: the user may not add
    /// to the project, or the request does not read.
    #[error("the task was refused: HTTP {0}")]
    Refused(StatusCode),
    /// The request failed, or document storage failed or answered with
    /// something unusable: worth trying again.
    #[error("{0}")]
    Unavailable(rootcause::Report),
}

impl DocumentStorageServiceClient {
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
        if status.is_client_error() {
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
}
