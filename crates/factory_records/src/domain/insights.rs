//! What insights read from a person's Runs: the Runs themselves, and their
//! Steps and Gates summed up by entry.

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{EntryId, RunId, RunOutcome};

/// The Runs insights read: one person's, created in `[from, to)`, of one
/// Flow or of all of them, and at most `cap` of them, the newest.
#[derive(Debug, Clone, PartialEq)]
pub struct InsightsScope {
    /// Whose Runs.
    pub user_id: MacroUserIdStr<'static>,
    /// The earliest creation time read.
    pub from: DateTime<Utc>,
    /// The first creation time not read.
    pub to: DateTime<Utc>,
    /// The built-in Flow's name, or none for every Flow.
    pub flow: Option<String>,
    /// The most Runs read; past it the oldest in the range are left out.
    pub cap: u16,
}

/// One Run, as insights count it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunSummary {
    /// The Run.
    pub id: RunId,
    /// The first line of its request, cut to [`TITLE_CHARS`] characters.
    pub title: String,
    /// The built-in Flow it walks; none for a Run recorded before the Flow
    /// was kept.
    pub flow: Option<String>,
    /// When it was created.
    pub created_at: DateTime<Utc>,
    /// How and when it ended; none while it goes.
    pub end: Option<(RunOutcome, DateTime<Utc>)>,
}

/// How many characters of a request's first line a [`RunSummary`] keeps.
pub const TITLE_CHARS: i32 = 120;

/// A spread of durations: how many, their median and 90th percentile, in
/// seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DurationSpread {
    /// How many durations.
    pub count: u32,
    /// The median.
    pub median_seconds: f64,
    /// The 90th percentile.
    pub p90_seconds: f64,
}

/// One entry's Steps across the Runs read.
#[derive(Debug, Clone, PartialEq)]
pub struct EntrySummary {
    /// The entry.
    pub entry: EntryId,
    /// Its Steps.
    pub steps: u32,
    /// The Runs that visited it.
    pub runs: u32,
    /// The Runs that visited it more than once.
    pub revisited_runs: u32,
    /// How long its Steps that ended took; none if none did. A Step a
    /// cancelled Run stopped is left out.
    pub duration: Option<DurationSpread>,
    /// The Gates its Steps opened.
    pub gates_opened: u32,
    /// How many of those a person answered.
    pub gates_answered: u32,
    /// How long people took to answer; none if nobody did.
    pub gate_wait: Option<DurationSpread>,
}

/// What a count of an entry's Steps counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryCountKind {
    /// Steps that ended with this outcome.
    Outcome,
    /// Gates answered with this answer.
    Answer,
}

/// How many of an entry's Steps ended with one outcome, or how many of its
/// Gates got one answer.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryCount {
    /// The entry.
    pub entry: EntryId,
    /// Whether it counts outcomes or answers.
    pub kind: EntryCountKind,
    /// The outcome or the answer, as the Flow names it.
    pub value: String,
    /// How many.
    pub count: u32,
}

/// A Run where an entry failed or was visited more than once.
#[derive(Debug, Clone, PartialEq)]
pub struct EntryTrouble {
    /// The entry.
    pub entry: EntryId,
    /// The Run.
    pub run_id: RunId,
    /// Whether one of the entry's Steps in the Run failed.
    pub failed: bool,
    /// How many times the Run visited the entry.
    pub visits: u32,
}

/// An agent Step's session, for reading what its agent used.
#[derive(Debug, Clone, PartialEq)]
pub struct StepSessionRef {
    /// The Run.
    pub run_id: RunId,
    /// The Step's entry.
    pub entry: EntryId,
    /// The agent session the Step worked in.
    pub session_id: Uuid,
}

/// What insights read about the Runs in an [`InsightsScope`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct InsightsRecords {
    /// The Runs, newest first.
    pub runs: Vec<RunSummary>,
    /// Their Steps, summed up by entry.
    pub entries: Vec<EntrySummary>,
    /// Their Steps' outcomes and their Gates' answers, by entry.
    pub counts: Vec<EntryCount>,
    /// For each entry, the newest Runs where it failed or was visited more
    /// than once, at most [`TROUBLE_RUNS_PER_ENTRY`] each.
    pub trouble: Vec<EntryTrouble>,
    /// The sessions of their agent Steps.
    pub sessions: Vec<StepSessionRef>,
}

/// How many Runs [`InsightsRecords::trouble`] keeps for each entry.
pub const TROUBLE_RUNS_PER_ENTRY: i64 = 5;

/// Reads what insights show about a person's Runs.
pub trait RunInsightsRepository: Send + Sync + 'static {
    /// The error type returned by reads.
    type Err: std::error::Error + Send + Sync + 'static;

    /// What insights read about the Runs in `scope`, all from one snapshot.
    fn read_insights(
        &self,
        scope: &InsightsScope,
    ) -> impl Future<Output = Result<InsightsRecords, Self::Err>> + Send;
}
