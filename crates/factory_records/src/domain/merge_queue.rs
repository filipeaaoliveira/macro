//! The merge queue: the Runs that reached their approved merge into a base
//! branch, in the order they first reached it, so Runs merge to one base one
//! after the other; and each update of a Run's branch with its base.

use chrono::TimeDelta;

use crate::domain::models::{RunId, StepKey};

/// How long a Run that left its merge, to bring its branch up to date with
/// its base and pass its checks again, keeps its place in the queue: the
/// Runs behind it wait for it meanwhile, rather than merge first and leave
/// it behind once more. Past it, they go on without it, and it comes back
/// to its place when it reaches its merge again.
pub const QUEUE_PLACE_KEPT: TimeDelta = TimeDelta::minutes(90);

/// An update Step's merge of its base into the Run's branch, kept before the
/// agent is asked to resolve a conflict, so a retry goes on the way the Step
/// started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchUpdate {
    /// The base's head commit merged into the branch.
    pub base_sha: String,
    /// The branch's head commit the update started from.
    pub from_head: String,
    /// Whether GitHub could not merge them: the agent resolves the conflict.
    pub conflict: bool,
    /// The branch's head commit GitHub's merge made, once seen; none for a
    /// conflict, which GitHub merges nothing for.
    pub updated_head: Option<String>,
}

/// The merge queue of each base branch, and the updates of Runs' branches.
pub trait MergeQueueRepository: Send + Sync + 'static {
    /// The error type returned by reads and writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// A Run's hold on its base's merge turn: no other Run of that base
    /// takes it until this is dropped, on whatever path that is.
    type Turn: Send + 'static;

    /// Take the merge turn of the base `run_id` works on, unless another
    /// Run holds it: none then. One Run of a base holds it at a time, from
    /// reading that no Run is ahead of it to its merge's end, so a Run that
    /// comes back to its place meanwhile waits for that merge rather than
    /// merge beside it. A Run that works on no repository takes a turn no
    /// other Run waits for. A turn held past a few minutes, as by a worker
    /// that stopped answering, is let go.
    fn take_merge_turn(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Option<Self::Turn>, Self::Err>> + Send;

    /// Put the Run `run_id` in the queue of the base branch it works on, at
    /// the end, or, when it was in it before, back at its place. A base is
    /// its repository's URL, lower-cased, trimmed and without a `.git`
    /// ending, and its branch. A Run that works on no repository is put in
    /// none.
    fn enter_merge_queue(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The Runs ahead of `run_id` in its base's queue, first first: those
    /// that entered it before it (in the order they entered, which a serial
    /// keeps total even for Runs that entered in the same instant), have not ended, and are at their merge or
    /// left it less than `place_kept` ago. Empty for a Run not in a queue.
    fn merge_queue_ahead(
        &self,
        run_id: RunId,
        place_kept: TimeDelta,
    ) -> impl Future<Output = Result<Vec<RunId>, Self::Err>> + Send;

    /// Record that the Run `run_id` left its merge without merging; it keeps
    /// its place. Leaving again keeps the first time it left.
    fn leave_merge_queue(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Take the Run `run_id`, which merged, out of its base's queue.
    fn drop_from_merge_queue(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The update the Step `key` keeps, if it kept one.
    fn branch_update(
        &self,
        key: StepKey,
    ) -> impl Future<Output = Result<Option<BranchUpdate>, Self::Err>> + Send;

    /// Keep `update` for the Step `key`, unless it keeps one already: the
    /// update it keeps, either way.
    fn keep_branch_update(
        &self,
        key: StepKey,
        update: &BranchUpdate,
    ) -> impl Future<Output = Result<BranchUpdate, Self::Err>> + Send;

    /// Replace the Step `key`'s update with `update`, asked again because
    /// GitHub's merge of the one kept never showed: only an update with no
    /// conflict and no head GitHub's merge made is replaced, as nothing was
    /// asked of an agent for it yet. The update it keeps, either way.
    fn restart_branch_update(
        &self,
        key: StepKey,
        update: &BranchUpdate,
    ) -> impl Future<Output = Result<BranchUpdate, Self::Err>> + Send;

    /// The head of the Run `run_id`'s branch its review last passed on, or
    /// GitHub's merge of its base into that head since; none before a
    /// review of it passed.
    fn reviewed_head(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// Keep `head` as the head of the Run `run_id`'s branch its review last
    /// passed on, replacing the one kept: when a review round passes on it,
    /// or once GitHub merged the base into that head and nothing else came
    /// in. Nothing else writes it, so an update asked again from a head
    /// someone pushed never makes that head reviewed.
    fn set_reviewed_head(
        &self,
        run_id: RunId,
        head: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record the branch's head GitHub's merge made for the Step `key`'s
    /// update, which merged with no conflict.
    fn set_updated_head(
        &self,
        key: StepKey,
        head: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
