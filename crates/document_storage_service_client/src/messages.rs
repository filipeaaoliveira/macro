//! A document's discussions, read as an acting user: the comment threads on
//! it, through document storage's `/messages/document/{id}` routes.

#[cfg(test)]
mod test;

use messages::domain::models::MessageThread;
use messages::domain::ports::{MessagePage, MessageTimelineQuery};
use reqwest::StatusCode;
use uuid::Uuid;

use super::DocumentStorageServiceClient;
use crate::constants::MACRO_INTERNAL_USER_ID_HEADER_KEY;

/// Why a document's discussions could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ReadMessagesError {
    /// The user may not read the document, or it is gone: they read none
    /// of its discussions.
    #[error("the document is not there for the user: HTTP {0}")]
    NotThere(StatusCode),
    /// Document storage could not answer now: worth trying again.
    #[error("the discussions could not be read: {0}")]
    Unavailable(String),
}

impl DocumentStorageServiceClient {
    /// One page of the discussions on the document `document_id` that
    /// `query` selects, newest root first, each with a preview of its
    /// replies, as `user_id` reads them.
    #[tracing::instrument(skip(self, query), err)]
    pub async fn document_messages(
        &self,
        document_id: Uuid,
        user_id: &str,
        query: &MessageTimelineQuery,
    ) -> Result<MessagePage, ReadMessagesError> {
        let selection = serde_json::to_string(query)
            .map_err(|error| ReadMessagesError::Unavailable(error.to_string()))?;
        let request = self
            .client
            .get(format!("{}/messages/document/{document_id}", self.url))
            .query(&[("selection", selection)])
            .header(MACRO_INTERNAL_USER_ID_HEADER_KEY, user_id);
        read(request).await
    }

    /// The discussion on the document `document_id` whose first comment is
    /// `root`, with all its replies, as `user_id` reads it.
    #[tracing::instrument(skip(self), err)]
    pub async fn document_thread(
        &self,
        document_id: Uuid,
        user_id: &str,
        root: Uuid,
    ) -> Result<MessageThread, ReadMessagesError> {
        let request = self
            .client
            .get(format!(
                "{}/messages/document/{document_id}/threads/{root}",
                self.url
            ))
            .header(MACRO_INTERNAL_USER_ID_HEADER_KEY, user_id);
        read(request).await
    }
}

/// Send `request` and read its answer: a refusal (401, 403) and a missing
/// document (404) are not there for the user; anything else that fails is
/// worth trying again.
async fn read<T: serde::de::DeserializeOwned>(
    request: reqwest::RequestBuilder,
) -> Result<T, ReadMessagesError> {
    let unavailable = |error: reqwest::Error| ReadMessagesError::Unavailable(error.to_string());
    let response = request.send().await.map_err(unavailable)?;
    let status = response.status();
    let refused = [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::NOT_FOUND,
    ];
    if refused.contains(&status) {
        return Err(ReadMessagesError::NotThere(status));
    }
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(ReadMessagesError::Unavailable(format!(
            "HTTP {status}: {body}"
        )));
    }
    response.json().await.map_err(unavailable)
}
