//! [`UserDirectory`] over the authentication service, which owns accounts.

#[cfg(test)]
mod test;

use std::collections::HashSet;

use authentication_service_client::AuthServiceClient;
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::ports::UserDirectory;

/// Asks the authentication service which accounts exist.
#[derive(Clone)]
pub struct AuthUserDirectory {
    auth: AuthServiceClient,
}

impl AuthUserDirectory {
    /// The directory, over the authentication service's client.
    pub fn new(auth: AuthServiceClient) -> Self {
        Self { auth }
    }
}

/// The authentication service did not answer.
#[derive(Debug, thiserror::Error)]
#[error("the authentication service did not say whether the user exists: {0:?}")]
pub struct AuthUserDirectoryErr(rootcause::Report);

impl UserDirectory for AuthUserDirectory {
    type Err = AuthUserDirectoryErr;

    #[tracing::instrument(err, skip_all)]
    async fn user_exists(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<bool, AuthUserDirectoryErr> {
        let existing = self
            .auth
            .get_existing_users(&[user_id.as_ref()])
            .await
            .map_err(|error| AuthUserDirectoryErr(rootcause::report!("{error:#}")))?;
        Ok(names(&existing, user_id.as_ref()))
    }
}

/// Whether the ids the authentication service says exist name `user_id`. It
/// answers with the ids it stores, which it lowercases, so the comparison
/// ignores case.
fn names(existing: &HashSet<String>, user_id: &str) -> bool {
    existing.iter().any(|id| id.eq_ignore_ascii_case(user_id))
}
