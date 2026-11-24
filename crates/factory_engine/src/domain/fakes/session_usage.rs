//! The in-memory stand-in for the usage agent sessions' runtimes reported.

use factory_records::domain::session_usage::{SessionUsage, SessionUsageRepository};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FakeError, FakeRepo};

impl SessionUsageRepository for FakeRepo {
    type Err = FakeError;

    /// As the records keep it: each count the larger of the two, a cost
    /// once kept stays, and someone else's session is left alone.
    async fn record_session_usage(&self, usage: &SessionUsage) -> Result<(), FakeError> {
        if self.fails("record_session_usage") {
            return Err(FakeError);
        }
        let mut kept = self.session_usage.lock().expect("session usage");
        let Some(old) = kept.get_mut(&usage.session_id) else {
            kept.insert(usage.session_id, usage.clone());
            return Ok(());
        };
        if old.user_id != usage.user_id {
            return Ok(());
        }
        old.input_tokens = old.input_tokens.max(usage.input_tokens);
        old.output_tokens = old.output_tokens.max(usage.output_tokens);
        old.cache_read_tokens = old.cache_read_tokens.max(usage.cache_read_tokens);
        old.cache_write_tokens = old.cache_write_tokens.max(usage.cache_write_tokens);
        old.cost_usd = match (old.cost_usd, usage.cost_usd) {
            (Some(old), Some(new)) => Some(old.max(new)),
            (old, new) => old.or(new),
        };
        if usage.model.is_some() {
            old.model.clone_from(&usage.model);
        }
        Ok(())
    }

    async fn session_usage(
        &self,
        user: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<Vec<SessionUsage>, FakeError> {
        if self.fails("session_usage") {
            return Err(FakeError);
        }
        let kept = self.session_usage.lock().expect("session usage");
        Ok(sessions
            .iter()
            .filter_map(|session| kept.get(session))
            .filter(|usage| usage.user_id == *user)
            .cloned()
            .collect())
    }
}
