//! In-memory stand-ins for the Main Hold: the merges and holds, the watches
//! started, and the tasks filed.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::{
    BaseState, FactoryMerge, MainHold, MainHoldRepository, RequiredChecksState,
    TASK_CLAIM_STALE_AFTER,
};
use factory_records::domain::models::{Repository, RunId};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FakeError, FakeRepo};
use crate::domain::main_hold::{HoldTask, MainWatch};
use crate::domain::ports::{HoldTasks, MainWatches};

/// What [`FakeRepo`] keeps of the Main Hold, by base: `(url, branch)`.
#[derive(Default)]
pub(crate) struct FakeMain {
    /// Each merge, with its base.
    pub(crate) merges: Vec<((String, String), FactoryMerge)>,
    /// When each base was last seen green.
    pub(crate) last_green: BTreeMap<(String, String), DateTime<Utc>>,
    /// The hold on each base that is held.
    pub(crate) holds: BTreeMap<(String, String), MainHold>,
    /// When each claimed hold's task was claimed.
    pub(crate) claimed_at: BTreeMap<(String, String), DateTime<Utc>>,
    /// The bases whose required checks decide nothing, and why.
    pub(crate) required: BTreeMap<(String, String), RequiredChecksState>,
    /// The bases whose hold's task was filed in this red period.
    pub(crate) filed: std::collections::BTreeSet<(String, String)>,
    /// The id each held base's task is filed with, chosen by its first
    /// claim.
    pub(crate) task_keys: BTreeMap<(String, String), Uuid>,
}

fn key(base: &Repository) -> (String, String) {
    (base.url.clone(), base.base_branch.clone())
}

impl FakeRepo {
    pub(crate) fn main(&self) -> std::sync::MutexGuard<'_, FakeMain> {
        self.main.lock().expect("main")
    }
}

impl MainHoldRepository for FakeRepo {
    type Err = FakeError;

    async fn record_merge(
        &self,
        run_id: RunId,
        number: u64,
        url: &str,
        merged_at: DateTime<Utc>,
    ) -> Result<(), FakeError> {
        let repository = self
            .runs
            .lock()
            .expect("runs")
            .get(&run_id)
            .and_then(|run| run.repository.clone());
        let Some(repository) = repository else {
            return Ok(());
        };
        let mut main = self.main();
        if main.merges.iter().any(|(_, merge)| merge.run_id == run_id) {
            return Ok(());
        }
        main.merges.push((
            key(&repository),
            FactoryMerge {
                run_id,
                number,
                url: url.to_string(),
                merged_at,
            },
        ));
        Ok(())
    }

    async fn main_hold(&self, base: &Repository) -> Result<Option<MainHold>, FakeError> {
        Ok(self.main().holds.get(&key(base)).cloned())
    }

    async fn base_state(&self, base: &Repository) -> Result<BaseState, FakeError> {
        let main = self.main();
        Ok(BaseState {
            hold: main.holds.get(&key(base)).cloned(),
            required: main.required.get(&key(base)).copied(),
        })
    }

    async fn mark_required(
        &self,
        base: &Repository,
        why: RequiredChecksState,
    ) -> Result<(), FakeError> {
        let mut main = self.main();
        main.holds.remove(&key(base));
        main.claimed_at.remove(&key(base));
        main.filed.remove(&key(base));
        main.task_keys.remove(&key(base));
        main.required.insert(key(base), why);
        Ok(())
    }

    async fn recent_merges(
        &self,
        base: &Repository,
        limit: u16,
    ) -> Result<Vec<FactoryMerge>, FakeError> {
        let mut merges: Vec<FactoryMerge> = self
            .main()
            .merges
            .iter()
            .filter(|(merged_to, _)| *merged_to == key(base))
            .map(|(_, merge)| merge.clone())
            .collect();
        merges.sort_by_key(|merge| std::cmp::Reverse(merge.merged_at));
        merges.truncate(usize::from(limit));
        Ok(merges)
    }

    async fn hold_main(
        &self,
        base: &Repository,
        head_sha: &str,
        failed: &str,
        seen_at: DateTime<Utc>,
    ) -> Result<MainHold, FakeError> {
        let mut main = self.main();
        main.required.remove(&key(base));
        let hold = main.holds.entry(key(base)).or_insert_with(|| MainHold {
            since: seen_at,
            head_sha: String::new(),
            failed: String::new(),
            task_id: None,
            task_claimed: false,
        });
        hold.head_sha = head_sha.to_string();
        hold.failed = failed.to_string();
        Ok(hold.clone())
    }

    async fn clear_main_hold(
        &self,
        base: &Repository,
        seen_at: DateTime<Utc>,
    ) -> Result<(), FakeError> {
        let mut main = self.main();
        if main
            .holds
            .get(&key(base))
            .is_some_and(|hold| hold.since >= seen_at)
        {
            return Ok(());
        }
        main.holds.remove(&key(base));
        main.claimed_at.remove(&key(base));
        main.filed.remove(&key(base));
        main.task_keys.remove(&key(base));
        main.required.remove(&key(base));
        let last = main.last_green.entry(key(base)).or_insert(seen_at);
        *last = (*last).max(seen_at);
        Ok(())
    }

    async fn suspect_merges(
        &self,
        base: &Repository,
        limit: u16,
    ) -> Result<Vec<FactoryMerge>, FakeError> {
        let main = self.main();
        let since = main.last_green.get(&key(base)).copied();
        let mut merges: Vec<FactoryMerge> = main
            .merges
            .iter()
            .filter(|(merged_to, merge)| {
                *merged_to == key(base) && since.is_none_or(|since| merge.merged_at > since)
            })
            .map(|(_, merge)| merge.clone())
            .collect();
        merges.sort_by_key(|merge| merge.merged_at);
        let skip = merges.len().saturating_sub(usize::from(limit));
        Ok(merges.split_off(skip))
    }

    async fn claim_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Option<Uuid>, FakeError> {
        let mut main = self.main();
        let stale = main
            .claimed_at
            .get(&key(base))
            .is_some_and(|claimed| now - *claimed > TASK_CLAIM_STALE_AFTER);
        let filed = main.filed.contains(&key(base));
        let claimed = match main.holds.get_mut(&key(base)) {
            Some(hold) if hold.since == since && !filed && (!hold.task_claimed || stale) => {
                hold.task_claimed = true;
                true
            }
            _ => false,
        };
        if !claimed {
            return Ok(None);
        }
        main.claimed_at.insert(key(base), now);
        Ok(Some(
            *main.task_keys.entry(key(base)).or_insert_with(Uuid::now_v7),
        ))
    }

    async fn release_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
    ) -> Result<(), FakeError> {
        let mut main = self.main();
        let filed = main.filed.contains(&key(base));
        if let Some(hold) = main.holds.get_mut(&key(base))
            && hold.since == since
            && !filed
        {
            hold.task_claimed = false;
        }
        Ok(())
    }

    async fn record_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
        task_id: Uuid,
    ) -> Result<(), FakeError> {
        let mut main = self.main();
        let recorded = match main.holds.get_mut(&key(base)) {
            Some(hold) if hold.since == since && hold.task_claimed => {
                hold.task_id = Some(task_id);
                true
            }
            _ => false,
        };
        if recorded {
            main.filed.insert(key(base));
        }
        Ok(())
    }
}

/// The watches asked for, in order; asking fails while `failing` is set.
#[derive(Clone, Default)]
pub(crate) struct FakeWatches {
    pub(crate) started: Arc<Mutex<Vec<MainWatch>>>,
    pub(crate) failing: Arc<Mutex<bool>>,
}

impl FakeWatches {
    pub(crate) fn started(&self) -> Vec<MainWatch> {
        self.started.lock().expect("watches").clone()
    }
}

impl MainWatches for FakeWatches {
    type Err = FakeError;

    async fn watch(&self, watch: &MainWatch) -> Result<(), FakeError> {
        if *self.failing.lock().expect("failing") {
            return Err(FakeError);
        }
        self.started.lock().expect("watches").push(watch.clone());
        Ok(())
    }
}

/// A task filed: its owner, the task it was filed beside, and the task.
pub(crate) type FiledTask = (String, Option<Uuid>, HoldTask);

/// A comment on a task: as whom, on which, on what passage, and what it
/// says.
pub(crate) type TaskComment = (String, Uuid, String, String);

/// The tasks filed; filing fails while `failing` is set.
#[derive(Clone, Default)]
pub(crate) struct FakeHoldTasks {
    pub(crate) filed: Arc<Mutex<Vec<FiledTask>>>,
    /// The id each filing was asked for, in order, failed ones too.
    pub(crate) ids: Arc<Mutex<Vec<Uuid>>>,
    pub(crate) failing: Arc<Mutex<bool>>,
    /// Told each time a task is filed.
    pub(crate) filing: Arc<tokio::sync::Notify>,
    /// The comments made on tasks.
    pub(crate) comments: Arc<Mutex<Vec<TaskComment>>>,
}

impl FakeHoldTasks {
    pub(crate) fn filed(&self) -> Vec<FiledTask> {
        self.filed.lock().expect("filed").clone()
    }
}

impl HoldTasks for FakeHoldTasks {
    type Err = FakeError;

    async fn file(
        &self,
        owner: &MacroUserIdStr<'static>,
        beside: Option<Uuid>,
        task: &HoldTask,
        task_id: Uuid,
    ) -> Result<Uuid, FakeError> {
        self.ids.lock().expect("ids").push(task_id);
        if *self.failing.lock().expect("failing") {
            return Err(FakeError);
        }
        self.filed
            .lock()
            .expect("filed")
            .push((owner.as_ref().to_string(), beside, task.clone()));
        self.filing.notify_one();
        Ok(task_id)
    }

    async fn comment(
        &self,
        owner: &MacroUserIdStr<'static>,
        task_id: Uuid,
        quote: &str,
        text: &str,
    ) -> Result<(), FakeError> {
        self.comments.lock().expect("comments").push((
            owner.as_ref().to_string(),
            task_id,
            quote.to_string(),
            text.to_string(),
        ));
        Ok(())
    }
}
