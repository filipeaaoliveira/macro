//! Break records: the evidence that a new test fails when the code it
//! covers breaks. A writer, or a fixer, reports one break for each test it
//! adds: an edit to a file that should make the test fail, and the command
//! that runs the test. The checks round of its Step runs each break on the
//! commit whose checks passed, and records how it went.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::domain::models::RunId;

/// The most bytes a break's test id, file, texts or command may hold (the
/// `factory_break_record` checks).
pub const MAX_BREAK_FIELD_BYTES: usize = 1_024;

/// The most bytes of a break's run its record keeps (the
/// `factory_break_record.output` check).
pub const MAX_BREAK_OUTPUT_BYTES: usize = 4_096;

/// How a break went.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
    strum::AsRefStr,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum BreakStatus {
    /// The test passed without the break and failed with it.
    Verified,
    /// The test passed with the break too: it does not catch it.
    NotReproduced,
    /// The test failed without the break: it does not pass here, so the
    /// break proves nothing.
    NotRun,
    /// The break's file is not in the commit, or does not hold its old
    /// text, so nothing was broken.
    NotApplied,
    /// The line that reports it does not read as a break.
    Unreadable,
    /// The Step's time ran out first, or it reported more breaks than are
    /// run.
    Dropped,
    /// The factory ran no command where the Step's agent works, or ran no
    /// checks: the break is the agent's word only.
    Unchecked,
    /// The run ended before it said how the test went: the session's
    /// command was stopped or lost. A later attempt runs it again.
    Interrupted,
}

/// One break a Step's agent reported, and how it went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BreakRecord {
    /// The Step whose agent reported it.
    pub seq: u16,
    /// The test, as the agent named it; the whole line for one that does
    /// not read.
    pub test: String,
    /// The file the break edits, from the repository's root.
    pub file: String,
    /// The text the break replaces: its first place in the file.
    pub old_text: String,
    /// What it puts in its place.
    pub new_text: String,
    /// The command that runs the test, from the repository's root.
    pub command: String,
    /// How it went.
    pub status: BreakStatus,
    /// The commit it ran on; none when it did not run.
    pub checked_commit: Option<String>,
    /// The end of what the test printed with the break, or without it when
    /// that run failed; or why it did not run.
    pub output: String,
    /// When it was recorded.
    pub recorded_at: DateTime<Utc>,
}

/// A break to record: [`BreakRecord`] without its Step and time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewBreakRecord {
    /// The test.
    pub test: String,
    /// The file it edits.
    pub file: String,
    /// The text it replaces.
    pub old_text: String,
    /// What it puts in its place.
    pub new_text: String,
    /// The command that runs the test.
    pub command: String,
    /// How it went.
    pub status: BreakStatus,
    /// The commit it ran on.
    pub checked_commit: Option<String>,
    /// The end of the run's output, or why it did not run.
    pub output: String,
}

/// The break records of some Runs, by Run, each Run's by Step, then in
/// the order they were recorded.
pub type RunsBreakRecords = BTreeMap<RunId, Vec<BreakRecord>>;
