//! Reading a document's discussions (its comment threads) as a user.

use chrono::{DateTime, Utc};
use messages::domain::ports::{MessagePage, MessageTimelineQuery};

use crate::DocumentStorageServiceClient;
use crate::constants::{INTERNAL_API_KEY_HEADER, INTERNAL_MACRO_USER_ID_HEADER};

/// The most discussions one read returns, the most the message service
/// serves.
const PAGE_SIZE: u16 = 100;

impl DocumentStorageServiceClient {
    /// The newest unanchored discussions on the document `document_id` with
    /// a comment or reply written at or after `after`, as `user_id` reads
    /// them: newest first, at most one page, each with a preview of its
    /// replies.
    #[tracing::instrument(skip(self, user_id), err)]
    pub async fn document_discussions_after(
        &self,
        document_id: &str,
        user_id: &str,
        after: DateTime<Utc>,
    ) -> anyhow::Result<MessagePage> {
        let selection = serde_json::to_string(&MessageTimelineQuery {
            anchored: Some(false),
            activity_after: Some(after),
            limit: Some(PAGE_SIZE),
            ..MessageTimelineQuery::default()
        })?;
        let page = self
            .external_client
            .get(format!("{}/messages/document/{document_id}", self.url))
            .query(&[("selection", selection)])
            .header(INTERNAL_API_KEY_HEADER, &self.internal_auth_key)
            .header(INTERNAL_MACRO_USER_ID_HEADER, user_id)
            .send()
            .await?
            .error_for_status()?
            .json::<MessagePage>()
            .await?;
        Ok(page)
    }
}
