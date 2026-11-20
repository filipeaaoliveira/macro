//! Self-improvement: the failures a team's Runs met under its Factory
//! Definition, kept until a self-improvement Run's pull request against the
//! Definition that groups them merges, and those self-improvement Runs, at
//! most one open per team.
//!
//! A failure is a live Scorer's judgment below [`PASS_SCORE`] or a Run
//! marked flawed. It is pending while no self-improvement Run holds it,
//! held while one is open, addressed once that Run's pull request merged,
//! and dismissed once a person rejected that Run (closed its pull request
//! unmerged, or cancelled it); a Run that ended without a pull request, or
//! could not start, lets its failures go back to pending.
//!
//! [`PASS_SCORE`]: crate::domain::evaluation::PASS_SCORE

use std::future::Future;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::models::{RunId, RunOutcome};

/// The built-in Flow a self-improvement Run walks, on its team's Definition
/// repository. Its Runs' own failures are never collected.
pub const IMPROVE_FLOW: &str = "improve";

/// The most bytes of a failure's reason that are kept.
pub const MAX_FAILURE_REASON_BYTES: usize = 4096;

/// A team's Factory Definition and who named it, whose self-improvement
/// Runs are theirs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionNaming {
    /// Its repository's `https://github.com/owner/name` URL.
    pub repo_url: String,
    /// Who named it; none for a Definition named before this was kept, or
    /// whose namer's account is gone.
    pub named_by: Option<MacroUserIdStr<'static>>,
}

/// Where a failure comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum FailureSource {
    /// A live Scorer judged the Run below the pass score.
    Scorer {
        /// The Scorer.
        name: String,
        /// Its score, from 0 to 1.
        score: f64,
    },
    /// The Run was marked flawed.
    Flawed,
}

impl FailureSource {
    /// What it is listed as: the Scorer's name, or `flawed`.
    pub fn label(&self) -> &str {
        match self {
            FailureSource::Scorer { name, .. } => name,
            FailureSource::Flawed => "flawed",
        }
    }
}

/// A failure to keep for a Run.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFailure {
    /// Where it comes from.
    pub source: FailureSource,
    /// Why, cut to [`MAX_FAILURE_REASON_BYTES`].
    pub reason: String,
}

/// A failure kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// The Run that failed.
    pub run_id: RunId,
    /// The Flow it walked.
    pub flow: String,
    /// Where the failure comes from.
    pub source: FailureSource,
    /// Why.
    pub reason: String,
    /// When it was kept.
    pub failed_at: DateTime<Utc>,
}

/// What a Run that ended tells about its failures.
#[derive(Debug, Clone, PartialEq)]
pub struct RunFailureFacts {
    /// Its owner.
    pub user_id: MacroUserIdStr<'static>,
    /// The Flow it walked.
    pub flow: String,
    /// The repository it worked on, if any.
    pub repo_url: Option<String>,
    /// The Definition repository it started under; none for a Run under
    /// none.
    pub definition_repo_url: Option<String>,
    /// The Scorers, Experiments and Benchmarks it kept from its
    /// Definition, as written.
    pub definition_evaluation: Option<serde_json::Value>,
    /// How it ended; none while it runs.
    pub outcome: Option<RunOutcome>,
    /// Whether the last cancel sent to it before it ended came from its
    /// owner in person: never for an agent's, a bot's under a Standing
    /// Role, an internal caller's or a program's.
    pub cancelled_in_person: bool,
    /// Why it was marked flawed, if it was.
    pub flaw_reason: Option<String>,
    /// The live Scorers' judgments of it: Scorer, score and reason.
    pub judgments: Vec<(String, f64, String)>,
}

/// A team's pending failures.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingFailures {
    /// How many.
    pub count: u32,
    /// When the oldest was kept.
    pub oldest: Option<DateTime<Utc>>,
    /// How many of each source ([`FailureSource::label`]), by label.
    pub by_source: Vec<(String, u32)>,
}

/// Where a self-improvement Run is.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Serialize,
    utoipa::ToSchema,
    strum::AsRefStr,
    strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum ImprovementState {
    /// Its Run is being recorded and started.
    Starting,
    /// Its Run started; it stays open while its pull request is open.
    Open,
    /// Its pull request merged: its failures are addressed.
    Merged,
    /// A person rejected it (its failures are dismissed), or its Run ended
    /// without a pull request (they went back to pending).
    Closed,
    /// Its Run could not start: its failures went back to pending.
    Refused,
}

impl ImprovementState {
    /// Whether it still holds its failures and its team's one place.
    pub fn is_open(self) -> bool {
        matches!(self, ImprovementState::Starting | ImprovementState::Open)
    }
}

/// A self-improvement Run of a team.
#[derive(Debug, Clone, PartialEq)]
pub struct Improvement {
    /// Its id.
    pub id: Uuid,
    /// The team.
    pub team_id: Uuid,
    /// The Definition repository it works on.
    pub repo_url: String,
    /// Its Run, once recorded; none while starting, or once the Run is
    /// gone.
    pub run_id: Option<RunId>,
    /// Where it is.
    pub state: ImprovementState,
    /// Why its Run could not start, when refused.
    pub refusal: Option<String>,
    /// When it began.
    pub created_at: DateTime<Utc>,
    /// When it was settled.
    pub settled_at: Option<DateTime<Utc>>,
    /// How many failures it holds or held.
    pub failures: u32,
}

/// What beginning a self-improvement Run did.
#[derive(Debug, Clone, PartialEq)]
pub enum ImprovementBegin {
    /// It is kept as starting, holding these failures, oldest first.
    Claimed(Vec<Failure>),
    /// One of the team's is open already: nothing was kept.
    AlreadyOpen,
    /// The team has no pending failure: nothing was kept.
    NoFailures,
}

/// How a self-improvement Run is settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settlement {
    /// Its pull request merged: its failures are addressed.
    Merged,
    /// Its Run ended without a pull request, or it no longer applies: its
    /// failures go back to pending.
    Closed,
    /// A person closed its pull request unmerged, or cancelled its Run: it
    /// is kept closed, and its failures are dismissed, never pending again.
    Rejected,
    /// Its Run could not start, for this reason.
    Refused(String),
}

/// Keeps failures and self-improvement Runs.
pub trait SelfImprovementRepository: Send + Sync + 'static {
    /// The error type returned by reads and writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// The Definition the team `team_id` names, and who named it; none
    /// when it names none.
    fn definition_naming(
        &self,
        team_id: Uuid,
    ) -> impl Future<Output = Result<Option<DefinitionNaming>, Self::Err>> + Send;

    /// What the Run `run_id` tells about its failures; none for no such
    /// Run.
    fn failure_facts(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Option<RunFailureFacts>, Self::Err>> + Send;

    /// Keep `failures` of the Run `run_id` for the team `team_id`, under
    /// its Definition `repo_url`. A failure kept already, by its Run and
    /// source, is kept as it was.
    fn record_failures(
        &self,
        team_id: Uuid,
        repo_url: &str,
        run_id: RunId,
        failures: &[NewFailure],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The team's pending failures under its Definition `repo_url`.
    fn pending_failures(
        &self,
        team_id: Uuid,
        repo_url: &str,
    ) -> impl Future<Output = Result<PendingFailures, Self::Err>> + Send;

    /// The team's latest self-improvement Run, open or not.
    fn latest_improvement(
        &self,
        team_id: Uuid,
    ) -> impl Future<Output = Result<Option<Improvement>, Self::Err>> + Send;

    /// The team's latest `limit` self-improvement Runs, newest first.
    fn recent_improvements(
        &self,
        team_id: Uuid,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Improvement>, Self::Err>> + Send;

    /// Keep a self-improvement Run `id` of the team as starting, on its
    /// Definition `repo_url`, holding its oldest `limit` pending failures
    /// under it: in one write, so two at once keep one.
    fn begin_improvement(
        &self,
        id: Uuid,
        team_id: Uuid,
        repo_url: &str,
        limit: u32,
    ) -> impl Future<Output = Result<ImprovementBegin, Self::Err>> + Send;

    /// Keep the Run `run_id` as the starting self-improvement Run `id`'s,
    /// which opens it; false when it is no longer starting.
    fn link_improvement_run(
        &self,
        id: Uuid,
        run_id: RunId,
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Settle the open self-improvement Run `id`: its failures are
    /// addressed when it merged, dismissed when rejected, else pending
    /// again. Settling one settled already changes nothing.
    fn settle_improvement(
        &self,
        id: Uuid,
        settlement: &Settlement,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;
}
