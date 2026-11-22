//! A usage log the tests set: what Runs' agent sessions used.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::token_budget::RunTokens;

/// A usage log that answers each read with the next of `reads` (`None`
/// fails the read), repeating the last once they run out, and keeps how
/// many reads were asked.
#[derive(Clone, Default)]
pub(crate) struct FakeRunTokens {
    reads: Arc<Mutex<VecDeque<Option<u64>>>>,
    last: Arc<Mutex<Option<u64>>>,
    pub(crate) asked: Arc<Mutex<usize>>,
    /// Every read finds the sessions recorded no usage at all.
    pub(crate) unmeasured: bool,
    /// What each session used, when reads sum what they are asked for
    /// rather than answer `reads`: a session asked twice counts twice.
    by_session: Option<Arc<HashMap<Uuid, u64>>>,
}

impl FakeRunTokens {
    /// A log whose reads answer `reads`, in order.
    pub(crate) fn reading(reads: impl IntoIterator<Item = Option<u64>>) -> Self {
        Self {
            reads: Arc::new(Mutex::new(reads.into_iter().collect())),
            ..Self::default()
        }
    }

    /// A log whose reads sum what each session asked for used, by
    /// `sessions`; a session it does not list used nothing.
    pub(crate) fn by_session(sessions: impl IntoIterator<Item = (Uuid, u64)>) -> Self {
        Self {
            by_session: Some(Arc::new(sessions.into_iter().collect())),
            ..Self::default()
        }
    }

    /// A log whose reads find the sessions recorded no usage at all.
    pub(crate) fn unmeasured() -> Self {
        Self {
            unmeasured: true,
            ..Self::reading([Some(0)])
        }
    }

    /// How many reads were asked.
    pub(crate) fn reads(&self) -> usize {
        *self.asked.lock().expect("asked")
    }
}

#[async_trait::async_trait]
impl RunTokens for FakeRunTokens {
    async fn used(
        &self,
        _owner: &MacroUserIdStr<'static>,
        sessions: &[Uuid],
    ) -> Result<Option<u64>, rootcause::Report> {
        *self.asked.lock().expect("asked") += 1;
        if let Some(used) = &self.by_session {
            return Ok(Some(
                sessions
                    .iter()
                    .map(|session| used.get(session).copied().unwrap_or(0))
                    .sum(),
            ));
        }
        let next = self.reads.lock().expect("reads").pop_front();
        let read = match next {
            Some(read) => {
                *self.last.lock().expect("last") = read;
                read
            }
            None => *self.last.lock().expect("last"),
        };
        let used = read.ok_or_else(|| rootcause::report!("the usage log is down"))?;
        Ok((!self.unmeasured).then_some(used))
    }
}
