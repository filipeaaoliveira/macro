//! How Runs are judged and compared: Scorers' judgments of finished Runs,
//! the Experiment arm each Run was assigned, and Benchmark suites, which
//! replay frozen tasks as trial Runs on one configuration.
//!
//! A trial's judgments are kept with the trial, never with the live
//! Scorers' judgments, so a benchmark does not move their numbers.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::insights::InsightsScope;
use crate::domain::models::{AgentName, EntryId, Outcome, Role, RunId, RunOutcome};

/// The built-in Flow a Benchmark's trials walk. Its Runs are trials only:
/// live insights and live Scorers leave them out.
pub const BENCHMARK_FLOW: &str = "benchmark";

/// The score at or above which a Scorer's judgment counts as a pass.
pub const PASS_SCORE: f64 = 0.5;

/// The most bytes of a judgment's reason that are kept.
pub const MAX_REASON_BYTES: usize = 4096;

/// The most bytes of the id of the model a judgment records.
pub const MAX_MODEL_BYTES: usize = 200;

/// How many suites [`EvaluationRecords::suites`] keeps: the newest.
pub const SUITES_READ: i64 = 20;

/// The most trials one person may have running at once, across their
/// suites: a suite that would pass it does not start.
pub const MAX_OPEN_TRIALS: u16 = 20;

/// How long a suite with no trial yet counts as running: its trials are
/// being started.
pub const SUITE_STARTING_SECS: f64 = 300.0;

/// The Agents a configuration gives Roles in place of the agent
/// definition's first; a Role it leaves out keeps the definition's.
pub type AgentChoices = BTreeMap<Role, AgentName>;

/// The arm of an Experiment a Run was assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunArm {
    /// The Experiment.
    pub experiment: String,
    /// The arm.
    pub arm: String,
    /// The Agents the arm gives the Run's Roles.
    pub agents: AgentChoices,
}

/// A Scorer's judgment of one Run or one trial.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgment {
    /// The Scorer.
    pub scorer: String,
    /// From 0 to 1.
    pub score: f64,
    /// Why, cut to [`MAX_REASON_BYTES`].
    pub reason: String,
    /// The model that judged.
    pub model: String,
}

/// One trial of a Benchmark suite: one frozen task, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrialKey {
    /// The suite.
    pub suite_id: Uuid,
    /// The frozen task.
    pub task: String,
}

/// The tokens a trial's agents used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrialTokens {
    /// Input tokens, besides those read from or written to the prompt cache.
    pub input: u64,
    /// Output tokens.
    pub output: u64,
    /// Input tokens read from the prompt cache.
    pub cache_read: u64,
    /// Input tokens written to the prompt cache.
    pub cache_write: u64,
}

/// A trial's result, once its Run ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrialResult {
    /// The mechanical score, from 0 to 1.
    pub score: f64,
    /// The tokens its agents used.
    pub tokens: TrialTokens,
}

/// One of a Run's Steps, as a judge reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoringStep {
    /// The Step's entry.
    pub entry: EntryId,
    /// How it ended; none while it runs.
    pub outcome: Option<Outcome>,
    /// What it produced; none while it runs.
    pub output: Option<String>,
    /// The agent session it worked in, if an agent did it.
    pub session_id: Option<Uuid>,
}

/// The trial a Run is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectTrial {
    /// The trial.
    pub key: TrialKey,
    /// The Benchmark its suite replays.
    pub benchmark: String,
    /// Whether it has its result already.
    pub scored: bool,
    /// Whether its Run's branch was deleted once it was scored.
    pub branch_deleted: bool,
}

/// A finished Run, as its judges and its trial's result read it.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoringSubject {
    /// The Run.
    pub run_id: RunId,
    /// Its owner: judges and usage reads run as them.
    pub user_id: MacroUserIdStr<'static>,
    /// The built-in Flow it walked.
    pub flow: Option<String>,
    /// What it was asked to do.
    pub request: String,
    /// The GitHub repository it worked on, if any.
    pub repo_url: Option<String>,
    /// The branch it started from and proposed its change to, with the
    /// repository.
    pub base_branch: Option<String>,
    /// How it ended; none while it goes.
    pub outcome: Option<RunOutcome>,
    /// Its Steps, in order.
    pub steps: Vec<ScoringStep>,
    /// The trial it is, if a Benchmark suite started it.
    pub trial: Option<SubjectTrial>,
    /// The Scorers that judged it already: into the live judgments, or into
    /// its trial's.
    pub judged: Vec<String>,
}

/// A suite to record before its trials start.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSuite<'a> {
    /// Its id.
    pub id: Uuid,
    /// Who started it: its trials run as them.
    pub user_id: &'a MacroUserIdStr<'static>,
    /// The Benchmark it replays.
    pub benchmark: &'a str,
    /// The configuration its trials run on.
    pub agents: &'a AgentChoices,
    /// How many trials it starts, held to [`MAX_OPEN_TRIALS`] with the
    /// person's running ones.
    pub trials: u16,
}

/// What recording a new suite came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuiteInsert {
    /// The suite is kept.
    Inserted,
    /// A suite of the same Benchmark of the person's still runs, so none
    /// was kept.
    Running,
    /// Its trials would take the person past [`MAX_OPEN_TRIALS`] running
    /// trials, counting this many running now, so none was kept.
    TooManyTrials(u32),
}

/// The Agents a Run's Steps start with, and whether it is a trial.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunAgents {
    /// The Agents its arms, or its suite, give its Roles.
    pub choices: AgentChoices,
    /// Whether it is a Benchmark trial: its Steps take no other Agent when
    /// the chosen one cannot start, so every trial runs on its
    /// configuration.
    pub trial: bool,
}

/// A Scorer's live judgments of the Runs read.
#[derive(Debug, Clone, PartialEq)]
pub struct ScorerTally {
    /// The Scorer.
    pub scorer: String,
    /// How many Runs it judged.
    pub judged: u32,
    /// How many scored at least [`PASS_SCORE`].
    pub passed: u32,
    /// Their mean score.
    pub mean_score: f64,
}

/// The Runs read that one arm of an Experiment took.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmTally {
    /// The Experiment.
    pub experiment: String,
    /// The arm.
    pub arm: String,
    /// The Agents the arm gives Roles.
    pub agents: AgentChoices,
    /// How many Runs it took.
    pub runs: u32,
    /// How many of them ended.
    pub ended: u32,
    /// How many ended `success`.
    pub succeeded: u32,
    /// How many had a Step of a Role the arm chooses for that another
    /// Agent took: the chosen one could not start, or the agent definition
    /// no longer has it.
    pub fell_back: u32,
}

/// One Scorer's live judgments of one arm's Runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ArmScore {
    /// The Experiment.
    pub experiment: String,
    /// The arm.
    pub arm: String,
    /// The Scorer.
    pub scorer: String,
    /// How many of the arm's Runs it judged.
    pub judged: u32,
    /// Their mean score.
    pub mean_score: f64,
}

/// One trial, as insights show it.
#[derive(Debug, Clone, PartialEq)]
pub struct TrialRecord {
    /// The frozen task.
    pub task: String,
    /// Its Run.
    pub run_id: RunId,
    /// How its Run ended; none while it goes.
    pub run_outcome: Option<RunOutcome>,
    /// Its result, once its Run ended and it was scored.
    pub result: Option<TrialResult>,
    /// The Scorers' judgments of it, by Scorer.
    pub scores: Vec<(String, f64)>,
}

/// One suite, with its trials.
#[derive(Debug, Clone, PartialEq)]
pub struct SuiteRecord {
    /// The suite.
    pub id: Uuid,
    /// The Benchmark it replays.
    pub benchmark: String,
    /// The configuration its trials ran on.
    pub agents: AgentChoices,
    /// When it started.
    pub created_at: DateTime<Utc>,
    /// Its trials, by task.
    pub trials: Vec<TrialRecord>,
}

/// What the evaluation insights read about an [`InsightsScope`]'s Runs and
/// the person's suites in its period.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EvaluationRecords {
    /// Each Scorer that judged one of the Runs.
    pub scorers: Vec<ScorerTally>,
    /// Each arm one of the Runs took.
    pub arms: Vec<ArmTally>,
    /// Each Scorer's judgments of each arm's Runs.
    pub arm_scores: Vec<ArmScore>,
    /// The person's suites started in the period, newest first, at most
    /// [`SUITES_READ`].
    pub suites: Vec<SuiteRecord>,
}

/// Keeps the Experiment arms, the Scorers' judgments and the Benchmark
/// suites. Every write is safe to repeat: a retried activity or request
/// records nothing twice.
pub trait EvaluationRepository: Send + Sync + 'static {
    /// The error type returned by reads and writes.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Record the arms a new Run was assigned.
    fn record_arms(
        &self,
        run_id: RunId,
        arms: &[RunArm],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The Agents the Run's arms, or its suite, give its Roles.
    fn run_agents(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<RunAgents, Self::Err>> + Send;

    /// The trial a Run is, if a Benchmark suite started it.
    fn trial_of(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Option<SubjectTrial>, Self::Err>> + Send;

    /// The commit a trial's write starts from; none for a Run that is not
    /// a trial.
    fn trial_base_commit(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Option<String>, Self::Err>> + Send;

    /// Record that a trial's Run's branch is deleted.
    fn record_branch_deleted(
        &self,
        trial: &TrialKey,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Keep `trials` as the number of trials a suite plans, once it stopped
    /// at a refused one, so the trials it never starts hold no place.
    fn set_planned_trials(
        &self,
        suite_id: Uuid,
        trials: u16,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Delete a suite none of whose trials started, so it is not listed; a
    /// suite with a trial is kept.
    fn delete_suite(&self, suite_id: Uuid) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The Run, as its judges read it; none when it is not recorded.
    fn scoring_subject(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Option<ScoringSubject>, Self::Err>> + Send;

    /// Keep a live Scorer's judgment of a Run, unless it has one.
    fn record_run_score(
        &self,
        run_id: RunId,
        judgment: &Judgment,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Keep a Scorer's judgment of a trial, unless it has one.
    fn record_trial_score(
        &self,
        trial: &TrialKey,
        judgment: &Judgment,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Keep a trial's result, unless it has one.
    fn record_trial_result(
        &self,
        trial: &TrialKey,
        result: &TrialResult,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record a suite, before its trials, unless a suite of the same
    /// Benchmark of the person's still runs or its trials would take them
    /// past [`MAX_OPEN_TRIALS`]. Two starts of one person are counted one
    /// after the other.
    fn insert_suite(
        &self,
        suite: &NewSuite<'_>,
    ) -> impl Future<Output = Result<SuiteInsert, Self::Err>> + Send;

    /// Record that `run_id` is `trial`, its write starting from
    /// `base_commit`, before the Run's workflow starts.
    fn insert_trial(
        &self,
        trial: &TrialKey,
        run_id: RunId,
        base_commit: &str,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// What the evaluation insights read about `scope`, from one snapshot.
    fn read_evaluation(
        &self,
        scope: &InsightsScope,
    ) -> impl Future<Output = Result<EvaluationRecords, Self::Err>> + Send;
}
