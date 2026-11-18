//! A document's discussions, read and posted to as an acting user: the
//! comment threads on it, through document storage's
//! `/messages/document/{id}` routes.

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

/// A comment to post on a document, as a new thread or a reply.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct NewDocumentComment<'a> {
    /// The comment's id: a UUIDv7 stamped within a day of now. Posting the
    /// same id again is refused as a conflict, so a retry posts it once.
    pub id: Uuid,
    /// The comment, as Macro markdown.
    pub content: &'a str,
    /// The thread it replies to, by its first comment's id; none starts a
    /// thread.
    pub thread_id: Option<Uuid>,
}

/// How posting a comment ended, short of an outage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentPosted {
    /// The comment is posted.
    Posted,
    /// A comment with its id was posted before (409).
    AlreadyPosted,
    /// The document, or the thread it replies to, is not there for the
    /// user (401, 403, 404).
    NotThere,
}

/// Why a comment could not be posted.
#[derive(Debug, thiserror::Error)]
pub enum PostCommentError {
    /// Document storage refused the comment itself (another 4xx): posting
    /// it again fails the same way.
    #[error("the comment was refused: HTTP {0}: {1}")]
    Refused(StatusCode, String),
    /// Document storage could not answer now: worth trying again.
    #[error("the comment could not be posted: {0}")]
    Unavailable(String),
}

impl DocumentStorageServiceClient {
    /// Post `comment` on the document `document_id` as `user_id`, who must
    /// be able to comment on it.
    #[tracing::instrument(skip(self, comment), fields(comment_id = %comment.id), err)]
    pub async fn post_document_comment(
        &self,
        document_id: Uuid,
        user_id: &str,
        comment: &NewDocumentComment<'_>,
    ) -> Result<CommentPosted, PostCommentError> {
        let response = self
            .client
            .post(format!("{}/messages/document/{document_id}", self.url))
            .header(MACRO_INTERNAL_USER_ID_HEADER_KEY, user_id)
            .json(comment)
            .send()
            .await
            .map_err(|error| PostCommentError::Unavailable(error.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(CommentPosted::Posted);
        }
        let not_there = [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
        ];
        if status == StatusCode::CONFLICT {
            return Ok(CommentPosted::AlreadyPosted);
        }
        if not_there.contains(&status) {
            return Ok(CommentPosted::NotThere);
        }
        let body = response.text().await.unwrap_or_default();
        if status.is_client_error() && status != StatusCode::TOO_MANY_REQUESTS {
            return Err(PostCommentError::Refused(status, body));
        }
        Err(PostCommentError::Unavailable(format!(
            "HTTP {status}: {body}"
        )))
    }

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
