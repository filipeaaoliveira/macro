//! In-memory stand-ins for the merge queue and the updates of Runs'
//! branches.

use std::collections::HashMap;

use chrono::{DateTime, TimeDelta, Utc};
use factory_records::domain::merge_queue::{BranchUpdate, MergeQueueRepository};
use factory_records::domain::models::{RunId, StepKey};

use super::{FakeError, FakeRepo};

/// A Run in a queue: its base, by `(url, branch)`, and when it left its
/// merge, if it did. Its place is its index.
pub(crate) struct FakeQueued {
    pub(crate) run_id: RunId,
    pub(crate) base: (String, String),
    pub(crate) left_at: Option<DateTime<Utc>>,
}

/// What [`FakeRepo`] keeps of the merge queues.
#[derive(Default)]
pub(crate) struct FakeQueue {
    /// Every Run in a queue, in the order they entered it.
    pub(crate) queued: Vec<FakeQueued>,
    /// Each update Step's update.
    pub(crate) updates: HashMap<StepKey, BranchUpdate>,
}

impl FakeRepo {
    pub(crate) fn queue(&self) -> std::sync::MutexGuard<'_, FakeQueue> {
        self.queue.lock().expect("queue")
    }

    /// The base of `run_id`: its repository's, or, for a Run the fake does
    /// not keep, one base shared by all such Runs.
    fn base_of(&self, run_id: RunId) -> (String, String) {
        self.runs
            .lock()
            .expect("runs")
            .get(&run_id)
            .and_then(|run| run.repository.clone())
            .map(|base| {
                let url = base.url.trim().to_lowercase();
                let url = url.strip_suffix(".git").unwrap_or(&url).to_string();
                (url, base.base_branch)
            })
            .unwrap_or_default()
    }

    fn has_ended(&self, run_id: RunId) -> bool {
        self.runs
            .lock()
            .expect("runs")
            .get(&run_id)
            .is_some_and(|run| run.end.is_some())
    }
}

impl MergeQueueRepository for FakeRepo {
    type Err = FakeError;

    async fn enter_merge_queue(&self, run_id: RunId) -> Result<(), FakeError> {
        let base = self.base_of(run_id);
        let mut queue = self.queue();
        match queue
            .queued
            .iter_mut()
            .find(|queued| queued.run_id == run_id)
        {
            Some(queued) => queued.left_at = None,
            None => queue.queued.push(FakeQueued {
                run_id,
                base,
                left_at: None,
            }),
        }
        Ok(())
    }

    async fn merge_queue_ahead(
        &self,
        run_id: RunId,
        place_kept: TimeDelta,
    ) -> Result<Vec<RunId>, FakeError> {
        let ahead: Vec<(RunId, Option<DateTime<Utc>>)> = {
            let queue = self.queue();
            let Some(place) = queue.queued.iter().position(|q| q.run_id == run_id) else {
                return Ok(Vec::new());
            };
            let base = &queue.queued[place].base;
            queue.queued[..place]
                .iter()
                .filter(|queued| &queued.base == base)
                .map(|queued| (queued.run_id, queued.left_at))
                .collect()
        };
        let now = Utc::now();
        Ok(ahead
            .into_iter()
            .filter(|(id, left_at)| {
                !self.has_ended(*id) && left_at.is_none_or(|left| left > now - place_kept)
            })
            .map(|(id, _)| id)
            .collect())
    }

    async fn leave_merge_queue(&self, run_id: RunId) -> Result<(), FakeError> {
        if let Some(queued) = self
            .queue()
            .queued
            .iter_mut()
            .find(|queued| queued.run_id == run_id && queued.left_at.is_none())
        {
            queued.left_at = Some(Utc::now());
        }
        Ok(())
    }

    async fn drop_from_merge_queue(&self, run_id: RunId) -> Result<(), FakeError> {
        self.queue().queued.retain(|queued| queued.run_id != run_id);
        Ok(())
    }

    async fn branch_update(&self, key: StepKey) -> Result<Option<BranchUpdate>, FakeError> {
        Ok(self.queue().updates.get(&key).cloned())
    }

    async fn keep_branch_update(
        &self,
        key: StepKey,
        update: &BranchUpdate,
    ) -> Result<BranchUpdate, FakeError> {
        Ok(self
            .queue()
            .updates
            .entry(key)
            .or_insert_with(|| update.clone())
            .clone())
    }

    async fn restart_branch_update(
        &self,
        key: StepKey,
        update: &BranchUpdate,
    ) -> Result<BranchUpdate, FakeError> {
        let mut queue = self.queue();
        let kept = queue.updates.get_mut(&key).ok_or(FakeError)?;
        if !kept.conflict && kept.updated_head.is_none() {
            *kept = update.clone();
        }
        Ok(kept.clone())
    }

    async fn set_updated_head(&self, key: StepKey, head: &str) -> Result<(), FakeError> {
        if let Some(update) = self.queue().updates.get_mut(&key)
            && !update.conflict
            && update.updated_head.is_none()
        {
            update.updated_head = Some(head.to_string());
        }
        Ok(())
    }
}
