//! What CI health, review quality, cost and autonomy read from a person's
//! Runs, and how a Run's `ci` Step keeps each wait on its pull request's
//! checks.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::insights::{InsightsScope, RunSummary};
use crate::domain::models::{RunId, StepKey};
use crate::domain::review_rounds::ReviewRoundOutcome;

/// How a wait on a pull request's checks ended, as its `ci` Step routes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiVerdict {
    /// The checks that count passed.
    Green,
    /// A check failed.
    Red,
    /// The checks did not finish in time.
    Timeout,
}

impl CiVerdict {
    /// The word the verdict is kept as, which is also the Step's outcome.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Red => "red",
            Self::Timeout => "timeout",
        }
    }

    /// The verdict kept as `word`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "green" => Some(Self::Green),
            "red" => Some(Self::Red),
            "timeout" => Some(Self::Timeout),
            _ => None,
        }
    }
}

/// How one CI job (a check run, or a commit status) ended on the commit a
/// wait read last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CiJobResult {
    /// It passed, or was skipped or neutral.
    Passed,
    /// It failed in its own work: the pull request's, unless the same job
    /// and step fail on other pull requests too.
    Failed,
    /// It failed setting up, before its work ran: a step that sets up the
    /// job, or a download that failed.
    Setup,
    /// It was cancelled, as when a newer push replaced its run.
    Cancelled,
}

impl CiJobResult {
    /// The word the result is kept as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Setup => "setup",
            Self::Cancelled => "cancelled",
        }
    }

    /// The result kept as `word`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "passed" => Some(Self::Passed),
            "failed" => Some(Self::Failed),
            "setup" => Some(Self::Setup),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// Whether the job turned its pull request red.
    pub fn is_red(self) -> bool {
        matches!(self, Self::Failed | Self::Setup)
    }
}

/// The most bytes a CI job's name, or its failed step's, is kept to.
pub const CI_NAME_BYTES: usize = 1024;

/// One CI job a wait saw finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiJob {
    /// Its name: a check run's, or a commit status's context. At most
    /// [`CI_NAME_BYTES`].
    pub name: String,
    /// How it ended.
    pub result: CiJobResult,
    /// For a job that failed, the first of its steps that failed, when it
    /// could be read. At most [`CI_NAME_BYTES`].
    pub failed_step: Option<String>,
}

/// How one wait of a `ci` Step on its pull request's checks ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiWait {
    /// The `ci` Step.
    pub key: StepKey,
    /// The commit whose checks it read last.
    pub head_sha: String,
    /// How it ended.
    pub verdict: CiVerdict,
    /// The jobs it saw finish on that commit; none for a timeout, or when
    /// they could not be read.
    pub jobs: Vec<CiJob>,
}

/// The Runs CI health and review quality read: those of an
/// [`InsightsScope`], of one repository or of all of them.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityScope {
    /// The person's Runs over the period, of a Flow or of all.
    pub runs: InsightsScope,
    /// The repository's URL, `https://github.com/owner/name`, or none for
    /// every repository.
    pub repository: Option<String>,
}

/// One Run, as CI health, review quality, cost and autonomy count it.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityRun {
    /// The Run.
    pub run: RunSummary,
    /// The repository it works on, by URL.
    pub repository: Option<String>,
    /// The page of the pull request it opened.
    pub pull_request: Option<String>,
    /// Whether it merged its pull request.
    pub merged: bool,
    /// Whether a person answered one of its Gates: not a Standing Rule, the
    /// stand-in or an agent.
    pub person_answered: bool,
    /// Whether its owner steered one of its Steps in the Step Terminal: a
    /// note, a pause, a resume or a stop.
    pub steered: bool,
    /// The agent sessions its Steps worked in.
    pub sessions: Vec<Uuid>,
}

/// A wait of a `ci` Step, as read back.
#[derive(Debug, Clone, PartialEq)]
pub struct CiWaitRecord {
    /// The Run.
    pub run_id: RunId,
    /// The `ci` Step's place.
    pub seq: u16,
    /// When the Step started waiting.
    pub started_at: DateTime<Utc>,
    /// When it ended.
    pub ended_at: DateTime<Utc>,
    /// How it ended.
    pub verdict: CiVerdict,
    /// The jobs it saw finish, by name.
    pub jobs: Vec<CiJob>,
}

/// A review round, as review quality counts it.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRoundRecord {
    /// The Run.
    pub run_id: RunId,
    /// The place of its first Step.
    pub seq: u16,
    /// When it started.
    pub started_at: DateTime<Utc>,
    /// How it ended; none while it reviews.
    pub outcome: Option<ReviewRoundOutcome>,
}

/// One reviewer's Step in a review round.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewerRecord {
    /// The Run.
    pub run_id: RunId,
    /// The reviewer's Step.
    pub seq: u16,
    /// Its round, by the place of the round's first Step.
    pub round_seq: u16,
    /// The reviewer's Role; none for a Step that claimed no session.
    pub role: Option<String>,
    /// The Agent that reviewed; none for a Step that claimed no session.
    pub agent: Option<String>,
}

/// A Blocker a reviewer found.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockerRecord {
    /// The Run.
    pub run_id: RunId,
    /// The reviewer's Step.
    pub seq: u16,
    /// Its round, by the place of the round's first Step.
    pub round_seq: u16,
    /// What is wrong, as the reviewer wrote it.
    pub problem: String,
}

/// What CI health, review quality, cost and autonomy read about the Runs
/// of a [`QualityScope`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QualityRecords {
    /// The Runs, newest first.
    pub runs: Vec<QualityRun>,
    /// Whether the scope had more Runs than its cap.
    pub capped: bool,
    /// The repositories the person's Runs of the period work on, whatever
    /// the scope's repository, by URL.
    pub repositories: Vec<String>,
    /// Their `ci` Steps' waits, in order.
    pub ci_waits: Vec<CiWaitRecord>,
    /// Their review rounds, in order.
    pub review_rounds: Vec<ReviewRoundRecord>,
    /// Their reviewers' Steps, in order.
    pub reviewers: Vec<ReviewerRecord>,
    /// The Blockers their reviewers found, in order.
    pub blockers: Vec<BlockerRecord>,
}

/// Reads what CI health, review quality, cost and autonomy show about a
/// person's Runs.
pub trait RunQualityRepository: Send + Sync + 'static {
    /// The error type returned by reads.
    type Err: std::error::Error + Send + Sync + 'static;

    /// What they read about the Runs in `scope`, all from one snapshot.
    fn read_quality(
        &self,
        scope: &QualityScope,
    ) -> impl Future<Output = Result<QualityRecords, Self::Err>> + Send;
}
