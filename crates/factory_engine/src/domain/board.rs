//! The factory's board: every Run a person can see at once, theirs or their
//! team's, by where each one is, a page at a time.

use chrono::{DateTime, Utc};
use factory_records::domain::board::BoardState;
use factory_records::domain::models::RunId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::RunDetail;

/// The most Runs a page of the board lists.
pub const BOARD_PAGE_MAX: u16 = 50;

/// The Runs a page of the board lists when the caller names no number.
pub const BOARD_PAGE_DEFAULT: u16 = 25;

/// The most characters of a search the board reads; the rest is left out.
pub const BOARD_SEARCH_CHARS: usize = 200;

/// Whose Runs the board lists.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BoardScope {
    /// The caller's own.
    #[default]
    Mine,
    /// The caller's and their team members' on tasks the caller can see.
    Team,
}

/// Which Runs a page of the board lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardFilter {
    /// Whose.
    pub scope: BoardScope,
    /// Only those there.
    pub state: Option<BoardState>,
    /// Only those of this built-in Flow, by name.
    pub flow: Option<String>,
    /// Only those on this repository, by its URL.
    pub repo_url: Option<String>,
    /// Only those whose task was in this Project when they started.
    pub project_id: Option<Uuid>,
    /// Only those whose request or task's name has these words in it.
    pub search: Option<String>,
    /// Where the page starts: the `next` of the page before.
    pub after: Option<BoardNext>,
    /// The most Runs it lists.
    pub limit: u16,
}

impl BoardFilter {
    /// `search` as the board reads it: trimmed, at most
    /// [`BOARD_SEARCH_CHARS`] characters, none when blank.
    pub fn search_words(search: Option<String>) -> Option<String> {
        let search = search?;
        let trimmed = search.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(trimmed.chars().take(BOARD_SEARCH_CHARS).collect())
    }

    /// `limit` as the board reads it: [`BOARD_PAGE_DEFAULT`] when none, at
    /// least one and at most [`BOARD_PAGE_MAX`].
    pub fn page_size(limit: Option<u16>) -> u16 {
        limit.unwrap_or(BOARD_PAGE_DEFAULT).clamp(1, BOARD_PAGE_MAX)
    }
}

/// Where the board's next page starts: past this Run, in the board's order
/// (newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardNext {
    /// When the last Run the page listed was created.
    pub created_at: DateTime<Utc>,
    /// Its id.
    pub id: RunId,
}

/// One Run on the board, with where it is.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardRun {
    /// Where it is.
    pub state: BoardState,
    /// The name of the task it works on; none without a task.
    pub task_name: Option<String>,
    /// The Run and its Steps.
    pub detail: RunDetail,
}

/// A page of the board.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BoardPage {
    /// The Runs, newest first: never a team member's Run on a task the
    /// caller cannot see.
    pub runs: Vec<BoardRun>,
    /// Where the next page starts, past the last Run listed; none on the
    /// last.
    pub next: Option<BoardNext>,
}
