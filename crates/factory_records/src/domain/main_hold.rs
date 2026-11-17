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

    /// Hold `base`, whose checks failed on `head_sha`: the failing checks
    /// `failed` replace the ones last seen, and a hold already set keeps its
    /// start and its task.
    fn hold_main(
        &self,
        base: &Repository,
        head_sha: &str,
        failed: &str,
    ) -> impl Future<Output = Result<MainHold, Self::Err>> + Send;

    /// Record that `base`'s checks passed on a read that started at
    /// `seen_at`, which ends any hold on it and its red period.
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

    /// Claim the filing of the task of the hold on `base` that started at
    /// `since`: whether this call claimed it. No other claim of the same red
    /// period succeeds, unless this one is released.
    fn claim_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Release a claim whose task could not be filed, so the next look files
    /// it.
    fn release_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record the task document `task_id` filed for the hold on `base` that
    /// started at `since`, while that hold lasts.
    fn record_hold_task(
        &self,
        base: &Repository,
        since: DateTime<Utc>,
        task_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
