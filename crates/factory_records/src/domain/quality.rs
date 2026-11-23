//! What CI health, review quality, cost and autonomy read from a person's
//! Runs, how a Run's `ci` Step keeps each wait on its pull request's
//! checks, and the reruns of flaky CI jobs it asks for and the jobs it
//! quarantines.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::insights::{InsightsScope, RunSummary};
use crate::domain::models::{RunId, StepKey};
use crate::domain::review_rounds::{DisputeRuling, FixStatus, ReviewRoundOutcome};

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
    /// It failed in its own work: the pull request's, unless the same test,
    /// or the same named step, fails on other pull requests too.
    Failed,
    /// It failed setting up, before its work ran: a step that sets up the
    /// job, or an action that could not be downloaded.
    Setup,
    /// It was cancelled: by a person, or, on a head a newer push replaced,
    /// by that push.
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
    /// For a job that failed, what failed finer than its step, when the
    /// check run annotated it: the first test that failed, by its file and
    /// name. At most [`CI_NAME_BYTES`].
    pub failure: Option<String>,
    /// Whether it ran on a head a newer push replaced while the Step
    /// waited, or since the Run's last wait, rather than on the head the
    /// wait read last: such a job is kept only cancelled.
    pub replaced: bool,
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
    /// The jobs it saw finish on that commit, and those a newer push
    /// cancelled on a head it replaced; none for a timeout, or when they
    /// could not be read. A name is kept once per head.
    pub jobs: Vec<CiJob>,
}

/// The head the Run's last kept `ci` wait read, with the jobs it kept
/// there: a job it kept finished before a newer push replaced the head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastCiWait {
    /// The commit whose checks it read last.
    pub head_sha: String,
    /// The names of the jobs it kept on that commit, in order.
    pub jobs: Vec<String>,
}

/// Why a `ci` Step took a failed CI job for flaky, and reran it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiRetryMatch {
    /// The Run's Factory Definition names it in `knownFlakes`.
    Known,
    /// It is quarantined in the Run's repository.
    Quarantined,
    /// The same failure turned other pull requests of the repository red.
    Record,
}

impl CiRetryMatch {
    /// The word it is kept as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Known => "known",
            Self::Quarantined => "quarantined",
            Self::Record => "record",
        }
    }

    /// The match kept as `word`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "known" => Some(Self::Known),
            "quarantined" => Some(Self::Quarantined),
            "record" => Some(Self::Record),
            _ => None,
        }
    }
}

/// How a rerun of a failed CI job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiRetryResult {
    /// The job passed when it ran again: a passed retry.
    Passed,
    /// It failed again, or its rerun never showed.
    Failed,
    /// GitHub would not rerun it.
    Refused,
}

impl CiRetryResult {
    /// The word it is kept as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Refused => "refused",
        }
    }

    /// The result kept as `word`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "passed" => Some(Self::Passed),
            "failed" => Some(Self::Failed),
            "refused" => Some(Self::Refused),
            _ => None,
        }
    }
}

/// A failed CI job a `ci` Step asks GitHub to rerun.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRetryRequest {
    /// The job's name. At most [`CI_NAME_BYTES`].
    pub name: String,
    /// The GitHub Actions job that failed.
    pub job_id: u64,
    /// The first of its steps that failed, when it was read. At most
    /// [`CI_NAME_BYTES`].
    pub failed_step: Option<String>,
    /// The first test its check run annotated as failed, when it did. At
    /// most [`CI_NAME_BYTES`].
    pub failure: Option<String>,
    /// Why it was taken for flaky.
    pub matched: CiRetryMatch,
}

/// A rerun a `ci` Step asked for, as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRetry {
    /// The job's name.
    pub name: String,
    /// The commit whose checks it reran.
    pub head_sha: String,
    /// The GitHub Actions job that failed, which its rerun replaces.
    pub job_id: u64,
    /// Why it was taken for flaky.
    pub matched: CiRetryMatch,
    /// How it ended; none while it runs.
    pub result: Option<CiRetryResult>,
    /// When it was asked for.
    pub requested_at: DateTime<Utc>,
}

/// A failure of a CI job that turned other pull requests of a repository
/// red: the job, its step and its test, as CI health tells failures apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlakyCiFailure {
    /// The job's name.
    pub name: String,
    /// The first of its steps that failed, when it was read.
    pub failed_step: Option<String>,
    /// The first test its check run annotated as failed, when it did.
    pub failure: Option<String>,
    /// On how many other pull requests it failed this way.
    pub pull_requests: u32,
}

/// What a `ci` Step reads to tell whether a red is flaky.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiFlakes {
    /// The jobs the Run's Factory Definition names as known flakes.
    pub known: Vec<String>,
    /// The jobs quarantined in the Run's repository.
    pub quarantined: Vec<String>,
    /// The failures that turned other pull requests of the repository red
    /// over the period read, the most widespread first.
    pub failures: Vec<FlakyCiFailure>,
}

/// A CI job to quarantine, with the id of the task filed for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCiQuarantine {
    /// The job's name.
    pub name: String,
    /// The id the task filed for it takes.
    pub task_id: Uuid,
}

/// A CI job quarantined in a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiQuarantine {
    /// The repository, by its URL in lower case.
    pub repository: String,
    /// The job's name.
    pub name: String,
    /// The Run whose rerun crossed the threshold, which files its task.
    pub run_id: RunId,
    /// The passed retries that quarantined it.
    pub passed_retries: u32,
    /// The id of the task filed for it.
    pub task_id: Uuid,
    /// Whether the task was filed yet.
    pub filed: bool,
    /// When it was quarantined.
    pub quarantined_at: DateTime<Utc>,
}

/// A rerun of a CI job by one of a scope's Runs, as CI health counts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRetryRecord {
    /// The Run.
    pub run_id: RunId,
    /// The job's name.
    pub name: String,
    /// How it ended; none while it runs.
    pub result: Option<CiRetryResult>,
}

/// The Runs CI health and review quality read: those of an
/// [`InsightsScope`], of one repository or of all of them.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityScope {
    /// The person's Runs over the period, of a Flow or of all.
    pub runs: InsightsScope,
    /// The repository's URL, `https://github.com/owner/name`, matched in
    /// any case, as GitHub names repositories; none for every repository.
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
    /// The agent sessions its Steps worked in, each once, those they left
    /// for a new one included.
    pub sessions: Vec<QualitySession>,
}

/// An agent session a Run's Steps worked in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualitySession {
    /// The session.
    pub id: Uuid,
    /// The bot that ran it; none for a session a Step left for a new one
    /// (the Step records only its current bot), or for a Step from before
    /// Steps recorded their bot.
    pub bot: Option<Uuid>,
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

/// A Blocker a reviewer found, or a finding dismissed as the repeat of
/// one it dropped.
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
    /// What the fix said of it, when a fix was given it.
    pub fix_status: Option<FixStatus>,
    /// How the next round ruled on it, when the fix disputed it.
    pub ruling: Option<DisputeRuling>,
    /// Whether it was dismissed rather than a Blocker: it counts only as
    /// dismissed.
    pub dismissed: bool,
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
    /// The CI jobs their `ci` Steps reran, in order.
    pub ci_retries: Vec<CiRetryRecord>,
    /// The CI jobs quarantined in the scope's repositories, by repository
    /// and name.
    pub ci_quarantines: Vec<CiQuarantine>,
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
