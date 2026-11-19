//! A usage log the tests set: what Runs' agent sessions used.

use std::collections::VecDeque;
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
}

impl FakeRunTokens {
    /// A log whose reads answer `reads`, in order.
    pub(crate) fn reading(reads: impl IntoIterator<Item = Option<u64>>) -> Self {
        Self {
            reads: Arc::new(Mutex::new(reads.into_iter().collect())),
            ..Self::default()
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
        _sessions: &[Uuid],
    ) -> Result<u64, rootcause::Report> {
        *self.asked.lock().expect("asked") += 1;
        let next = self.reads.lock().expect("reads").pop_front();
        let read = match next {
            Some(read) => {
                *self.last.lock().expect("last") = read;
                read
            }
            None => *self.last.lock().expect("last"),
        };
        read.ok_or_else(|| rootcause::report!("the usage log is down"))
    }
}
