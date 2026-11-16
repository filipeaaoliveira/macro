//! Stand-ins for the records and the usage log that insights read.

use std::sync::{Arc, Mutex};

use ai_usage::domain::session_usage::{SessionTokens, SessionUsageReader};
use factory_records::domain::insights::{InsightsRecords, InsightsScope, RunInsightsRepository};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[error("fake failure")]
pub(crate) struct FakeError;

/// Records that answer with `records` and keep the scopes they were asked.
pub(crate) struct FakeRecords {
    records: InsightsRecords,
    pub(crate) asked: Arc<Mutex<Vec<InsightsScope>>>,
}

impl FakeRecords {
    pub(crate) fn new(records: InsightsRecords) -> Self {
        Self {
            records,
            asked: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl RunInsightsRepository for FakeRecords {
    type Err = FakeError;

    async fn read_insights(&self, scope: &InsightsScope) -> Result<InsightsRecords, FakeError> {
        self.asked.lock().expect("asked").push(scope.clone());
        Ok(self.records.clone())
    }
}

/// A usage log that answers with `tokens` for the sessions asked, and keeps
/// who asked, for how many sessions.
pub(crate) struct FakeUsage {
    tokens: Vec<SessionTokens>,
    pub(crate) asked: Mutex<Vec<(MacroUserIdStr<'static>, usize)>>,
}

impl FakeUsage {
    pub(crate) fn new(tokens: Vec<SessionTokens>) -> Self {
        Self {
            tokens,
            asked: Mutex::new(Vec::new()),
        }
    }
}

impl SessionUsageReader for FakeUsage {
    type Err = FakeError;

    async fn session_tokens(
        &self,
        user: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<Vec<SessionTokens>, FakeError> {
        self.asked
            .lock()
            .expect("asked")
            .push((user.clone(), sessions.len()));
        Ok(self
            .tokens
            .iter()
            .filter(|tokens| sessions.contains(&tokens.session_id))
            .copied()
            .collect())
    }
}
