//! The Agents their owners' quota refused lately. A refusal for lack of
//! quota is remembered for [`QUOTA_COOL_DOWN`], per owner and Agent on its
//! bot (an Agent that names a vendor runs on several), and a
//! Step skips an Agent remembered so, recording it passed over as
//! [`NO_QUOTA_LATELY`], without opening a session for it: the refusal would
//! come again, after a prepare and a prompt.
//!
//! Neither the harness nor billing tells the factory ahead of a session
//! whether an Agent's quota is used up: only the refusal of a prepare or a
//! prompt does. So recovery is found by asking again: once the cool-down
//! passed, the next Step tries the Agent, and a start forgets the refusal,
//! while another refusal remembers it anew. A skip never renews it. The
//! Role's last Agent is never skipped, so a Step always asks one Agent at
//! least. The memory is the worker's own: another worker, or one that
//! restarted, learns of a refusal by meeting it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use macro_user_id::user_id::MacroUserIdStr;
use tokio::time::Instant;

use super::AgentBot;

/// How long an Agent its owner's quota refused is skipped.
pub const QUOTA_COOL_DOWN: Duration = Duration::from_secs(15 * 60);

/// Why a Step passed over an Agent it skipped, without asking, for a
/// refusal less than [`QUOTA_COOL_DOWN`] ago.
pub const NO_QUOTA_LATELY: &str = "no_quota_lately";

/// When each owner's quota last refused each Agent, shared by every clone.
#[derive(Debug, Clone, Default)]
pub(super) struct QuotaMemory {
    refused: Arc<Mutex<HashMap<(MacroUserIdStr<'static>, AgentBot), Instant>>>,
}

impl QuotaMemory {
    /// Remember that `owner`'s quota refused `agent` now.
    pub(super) fn refused(&self, owner: &MacroUserIdStr<'static>, agent: &AgentBot) {
        self.lock()
            .insert((owner.clone(), agent.clone()), Instant::now());
    }

    /// Forget a refusal of `agent` for `owner`: it started.
    pub(super) fn started(&self, owner: &MacroUserIdStr<'static>, agent: &AgentBot) {
        self.lock().remove(&(owner.clone(), agent.clone()));
    }

    /// Whether `owner`'s quota refused `agent` less than
    /// [`QUOTA_COOL_DOWN`] ago. Refusals older than that are forgotten.
    pub(super) fn refused_lately(&self, owner: &MacroUserIdStr<'static>, agent: &AgentBot) -> bool {
        let mut refused = self.lock();
        refused.retain(|_, at| at.elapsed() < QUOTA_COOL_DOWN);
        refused.contains_key(&(owner.clone(), agent.clone()))
    }

    /// The map, even after a panic while another holder had it: its
    /// entries are each whole.
    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<(MacroUserIdStr<'static>, AgentBot), Instant>> {
        self.refused.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
