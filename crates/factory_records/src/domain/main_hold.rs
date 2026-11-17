//! The Main Hold: a base branch the factory merges to whose required checks
//! fail, so no Run merges to it until they pass again; and the pull requests
//! Runs merged, which name the merges a red base may come from.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::domain::models::{Repository, RunId};

/// A hold on a base branch whose required checks failed. It lasts until the
/// checks on the base's head pass again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MainHold {
    /// When the base was first seen red, in this red period.
    pub since: DateTime<Utc>,
    /// The base's head commit, as the checks were last seen failing on it.
    pub head_sha: String,
    /// The checks that failed, one line each, with their pages.
    pub failed: String,
    /// The task filed with the merges made since the base was last green:
    /// its task document's id, once it is filed. None before then, or once
    /// the task document is deleted.
    pub task_id: Option<Uuid>,
    /// Whether the task was claimed for filing in this red period: a hold
    /// whose task is claimed never files another.
    #[serde(skip)]
    pub task_claimed: bool,
}

/// Why a base branch's required checks decide nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum RequiredChecksState {
    /// Which checks the base requires could not be read.
    Unread,
    /// The base requires no checks: nothing holds a merge to it.
    None,
}

impl RequiredChecksState {
    /// The state a stored name names.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "unread" => Some(Self::Unread),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// Its stored name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::None => "none",
        }
    }
}

/// What the factory last saw of a base branch: its hold while its checks
/// fail, and why its required checks decide nothing, when they do not.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseState {
    /// The hold, while its required checks fail.
    pub hold: Option<MainHold>,
    /// Why the last look's required checks decided nothing, if they did not.
    pub required: Option<RequiredChecksState>,
}

/// How long a claim of a hold's task may go without being filed before
/// another look may claim it again: past a look's own timeout, so only a
/// look that died between the claim and the filing is overtaken.
pub const TASK_CLAIM_STALE_AFTER: chrono::TimeDelta = chrono::TimeDelta::minutes(15);

/// A pull request a Run merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactoryMerge {
    /// The Run that merged it.
    pub run_id: RunId,
    /// Its number in the repository.
    pub number: u64,
    /// Its page on GitHub.
    pub url: String,
    /// When the Run saw it merged.
    pub merged_at: DateTime<Utc>,
}

/// The merges Runs made and the holds on the base branches they merged to,
/// each keyed by its base: a repository and its branch.
pub trait MainHoldRepository: Send + Sync + 'static {
    /// The error type returned by reads and writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Record that the Run `run_id` merged pull request `number`, at `url`,
    /// into the base branch it works on, as seen at `merged_at`. Recording
    /// it again changes nothing; a Run that works on no repository records
    /// none. `merged_at` is on the workers' clock, as the green looks'
    /// times are, so the two compare.
    fn record_merge(
        &self,
        run_id: RunId,
        number: u64,
        url: &str,
        merged_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The hold on `base`, if its checks were last seen failing.
    fn main_hold(
        &self,
        base: &Repository,
    ) -> impl Future<Output = Result<Option<MainHold>, Self::Err>> + Send;

    /// What was last seen of `base`: its hold, and whether its required
    /// checks could not be read.
    fn base_state(
        &self,
        base: &Repository,
    ) -> impl Future<Output = Result<BaseState, Self::Err>> + Send;

    /// Hold `base`, whose checks failed on `head_sha` on a read that
    /// started at `seen_at`: the failing checks `failed` replace the ones
    /// last seen, and a hold already set keeps its start and its task.
    fn hold_main(
        &self,
        base: &Repository,
        head_sha: &str,
        failed: &str,
        seen_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<MainHold, Self::Err>> + Send;

    /// Record that the checks `base` requires decide nothing, for `why`,
    /// and drop any hold on it: the caller keeps a hold that still stands.
    fn mark_required(
        &self,
        base: &Repository,
        why: RequiredChecksState,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that `base`'s checks passed on a read that started at
    /// `seen_at`, which ends any hold on it and its red period, unless the
    /// hold started at or after `seen_at`: a later red read stands.
    fn clear_main_hold(
        &self,
        base: &Repository,
        seen_at: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The pull requests Runs merged into `base` since its checks last
    /// passed, oldest first: at most the newest `limit`.
    fn suspect_merges(
        &self,
        base: &Repository,
        limit: u16,
    ) -> impl Future<Output = Result<Vec<FactoryMerge>, Self::Err>> + Send;

    /// The newest pull requests Runs merged into `base`, newest first: at
    /// most `limit`.
    fn recent_merges(
        &self,
        base: &Repository,
        limit: u16,
    ) -> impl Future<Output = Result<Vec<FactoryMerge>, Self::Err>> + Send;

    /// Claim the filing of the task of the hold on `base` that started at
    /// `since`, at `now` on the worker's clock: whether this call claimed
    /// it. No other claim of the same red period succeeds, unless this one
    /// is released, or is older than [`TASK_CLAIM_STALE_AFTER`] and its
    /// task was never filed. A task filed and deleted since is not filed
    /// again.
    fn claim_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Release a claim whose task could not be filed, so the next look files
    /// it; a claim whose task was filed is kept.
    fn release_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record the task document `task_id` filed for the hold on `base` that
    /// started at `since`, while that hold lasts: the red period's task is
    /// filed from then on.
    fn record_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
        task_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
