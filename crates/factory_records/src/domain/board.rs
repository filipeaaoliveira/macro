//! What the factory's board reads: Runs across tasks, one person's or
//! their team's, newest first, by where each one is.

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::{Run, RunId};

/// Where a Run is, as the board groups it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum::EnumString,
    strum::AsRefStr,
    utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum BoardState {
    /// It goes: a Step runs, or the next one is about to start.
    Running,
    /// It waits on its owner at a Gate no one answered yet: a Flow's Gate or
    /// an agent's question.
    Waiting,
    /// It is at its merge and the base branch it merges to is under a Main
    /// Hold, so it does not merge until the base's checks pass again.
    Held,
    /// It ended for a person to take over: `manual`, `blocked` or `failed`.
    Stopped,
    /// It ended as asked: `success`, or a person cancelled it.
    Finished,
}

/// Where a page of the board starts: past the Run created at `created_at`
/// with the id `id`, in the board's order (newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardCursor {
    /// The creation time of the last Run read.
    pub created_at: DateTime<Utc>,
    /// Its id.
    pub id: RunId,
}

/// The Runs a page of the board reads: `user_id`'s, and their team
/// members' when `team_id` names their team, narrowed by each filter set.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardQuery {
    /// The person the board is for.
    pub user_id: MacroUserIdStr<'static>,
    /// Their team, to read its members' Runs too; none for theirs alone.
    pub team_id: Option<Uuid>,
    /// Only Runs there.
    pub state: Option<BoardState>,
    /// Only Runs of this built-in Flow, by name.
    pub flow: Option<String>,
    /// Only Runs on this repository, by its `https://github.com/owner/name`
    /// URL.
    pub repo_url: Option<String>,
    /// Only Runs whose task was in this Project when they started.
    pub project_id: Option<Uuid>,
    /// Only Runs whose request, or whose task's name, has these words in
    /// it, in any case.
    pub search: Option<String>,
    /// The Flow entries that merge a Run's change: a Run whose latest Step
    /// is one of them is held while its base is.
    pub merge_entries: Vec<String>,
    /// Where the page starts; none for the first.
    pub after: Option<BoardCursor>,
    /// The most Runs read.
    pub limit: u16,
}

/// One Run the board read, with where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardRun {
    /// The Run.
    pub run: Run,
    /// Where it is.
    pub state: BoardState,
    /// The name of the task it works on; none without a task, or once the
    /// task is trashed.
    pub task_name: Option<String>,
    /// Whether it works on a task that is there: a task document, not
    /// trashed.
    pub task_live: bool,
}

/// The Runs that opened some pull requests, among those a board of
/// `user_id`'s reads: theirs, and their team members' when `team_id` names
/// their team.
#[derive(Debug, Clone, PartialEq)]
pub struct PullRequestRunsQuery {
    /// The person asking.
    pub user_id: MacroUserIdStr<'static>,
    /// Their team, to read its members' Runs too; none for theirs alone.
    pub team_id: Option<Uuid>,
    /// The pull requests' pages, as [`pull_request_key`] reads each.
    pub urls: Vec<String>,
    /// The Flow entries that merge a Run's change, as the board's.
    pub merge_entries: Vec<String>,
    /// The most Runs read.
    pub limit: u16,
}

/// The board's reads.
pub trait RunBoardRepository: Send + Sync + 'static {
    /// The error type returned by the repository.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The Runs `query` names, newest first, up to its limit.
    fn board_runs(
        &self,
        query: &BoardQuery,
    ) -> impl Future<Output = Result<Vec<BoardRun>, Self::Err>> + Send;

    /// The Runs whose pull request is one of `query`'s, each read as the
    /// board reads it, newest first, up to its limit.
    fn pull_request_runs(
        &self,
        query: &PullRequestRunsQuery,
    ) -> impl Future<Output = Result<Vec<BoardRun>, Self::Err>> + Send;
}

/// The page of a pull request as Runs are looked up by it: GitHub names
/// owners and repositories in any case, so in lower case.
pub fn pull_request_key(url: &str) -> String {
    url.to_lowercase()
}

/// `search` as a pattern `ILIKE` reads literally, anywhere in the text.
pub fn search_pattern(search: &str) -> String {
    let mut pattern = String::with_capacity(search.len() + 2);
    pattern.push('%');
    for character in search.chars() {
        if matches!(character, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(character);
    }
    pattern.push('%');
    pattern
}
