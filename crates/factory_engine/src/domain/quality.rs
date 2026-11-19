//! CI health, review quality, cost per merged pull request and the autonomy
//! rate of a person's Runs over a period, of one repository or of all.
//!
//! The records give the Runs, each `ci` Step's waits with the jobs they saw
//! finish, the review rounds with their reviewers and Blockers, and whether
//! a person stepped into a Run; agent usage comes per session. This module
//! classes each CI failure, counts review rounds and Blockers, and prices
//! the merged pull requests.
//!
//! A CI job's failure is the CI's setup when it failed setting up (as the
//! `ci` Step recorded it), and cancelled when it was cancelled, as a newer
//! push does to the runs of the head it replaces. A failure that names what
//! failed (the test its check run annotated, or else a step named for its
//! own work rather than a generic one such as "Run tests") is flaky when the
//! same job failed the same way on two or more pull requests of the period.
//! Every other failure is its own pull request's: two pull requests that
//! each broke a test under the same generic step, or a job whose step could
//! not be read, broke their own.

#[cfg(test)]
mod test;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use ai_usage::domain::session_usage::{SessionTokens, SessionUsageReader};
use chrono::{DateTime, NaiveDate, Utc};
use factory_records::domain::models::{RunId, RunOutcome};
use factory_records::domain::quality::{
    CI_NAME_BYTES, CiJob, CiJobResult, CiVerdict, QualityRecords, QualityRun, QualityScope,
};
use factory_records::domain::review_rounds::ReviewRoundOutcome;
use macro_user_id::user_id::MacroUserIdStr;
use serde::Serialize;
use uuid::Uuid;

use crate::domain::insights::{
    DurationStats, INSIGHTS_RUN_CAP, InsightsError, InsightsQuery, RunLink, TokenCounts,
    duration_stats,
};
use crate::domain::models::{CheckState, CodeRepository, FailureAnnotation, FinishedJob};

/// The most agent sessions one usage read sums up.
const SESSIONS_PER_READ: usize = 1000;

/// How many CI jobs [`CiHealth::jobs`] lists, the reddest first.
const JOBS_LISTED: usize = 50;

/// How many failures [`CiHealth::recurring`] and Blocker texts
/// [`ReviewQuality::recurring`] list.
const RECURRING_LISTED: usize = 20;

/// How many Runs a recurring failure or Blocker links to.
const RUNS_LINKED: usize = 5;

/// On how many pull requests the same failure must happen to be flaky,
/// rather than the pull request's own.
const FLAKY_PULL_REQUESTS: usize = 2;

// What CI health records.

/// Steps a CI job takes before its own work, by how their names start: a
/// failure in one is the CI's setup, not the pull request's. Only steps
/// GitHub Actions or a known action names: one named "Setup test database"
/// may run the pull request's own migrations.
const SETUP_STEPS: [&str; 8] = [
    "set up job",
    "initialize containers",
    "checkout",
    "run actions/checkout",
    "run actions/setup-",
    "run actions/cache",
    "cache dependencies",
    "install dependencies",
];

/// Toolchains a step named "Set up", "Setup" or "Install" and one of them
/// installs, as `actions/setup-node` does under the name "Setup Node.js".
const SETUP_TOOLCHAINS: [&str; 16] = [
    "node", "node.js", "nodejs", "python", "go", "java", "jdk", "rust", "bun", "deno", "ruby",
    "dotnet", ".net", "pnpm", "yarn", "uv",
];

/// The most bytes a failure's name is kept to: a test's file and title are
/// shorter, and more is likely log output.
const FAILURE_BYTES: usize = 200;

/// How a line of a failed job's output, or of an annotation, starts when
/// GitHub Actions could not set the job up: an action it could not download
/// or resolve, before any of the job's own work ran. Only these phrases
/// count, at the start of a line: a test's own output may say `404` or
/// name a network error, and that is its pull request's failure.
const SETUP_ERRORS: [&str; 5] = [
    "unable to download action",
    "unable to resolve action",
    "failed to download action",
    "failed to resolve action download info",
    "an action could not be found at the uri",
];

/// Words of a step's name that say it runs the project's own work as a
/// whole, as tests, a build or a linter: two pull requests failing in such
/// a step may have broken two different things.
const GENERIC_STEP_WORDS: [&str; 15] = [
    "test",
    "tests",
    "testing",
    "spec",
    "specs",
    "check",
    "checks",
    "lint",
    "clippy",
    "typecheck",
    "build",
    "compile",
    "e2e",
    "ci",
    "make",
];

/// Where GitHub Actions annotates its own errors, which name no test.
const ACTIONS_ANNOTATION_PATH: &str = ".github";

/// `text` cut to at most [`CI_NAME_BYTES`], on a character's boundary.
fn kept_name(text: &str) -> &str {
    cut(text, CI_NAME_BYTES)
}

/// `text` cut to at most `bytes`, on a character's boundary.
fn cut(text: &str, bytes: usize) -> &str {
    let mut end = text.len().min(bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Whether `line` starts with one of [`SETUP_ERRORS`], after an `Error:`
/// GitHub Actions may put first.
fn setup_error(line: &str) -> bool {
    let line = line.trim().to_lowercase();
    let line = line.strip_prefix("##[error]").unwrap_or(&line).trim_start();
    let line = line.strip_prefix("error:").unwrap_or(line).trim_start();
    SETUP_ERRORS.iter().any(|error| line.starts_with(error))
}

/// Whether the step `step` runs the project's own work as a whole, so its
/// name tells no failure from another: an unnamed `run` step, which GitHub
/// names `Run` and its command, or one named for tests, a build or a linter.
/// An action's step, `Run owner/name@ref`, is named for the action.
pub(crate) fn generic_step(step: &str) -> bool {
    let step = step.trim().to_lowercase();
    if let Some(command) = step.strip_prefix("run ") {
        let mut words = command.split_whitespace();
        let action = words
            .next()
            .is_some_and(|word| word.contains('/') && word.contains('@'))
            && words.next().is_none();
        if !action {
            return true;
        }
    }
    step.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| GENERIC_STEP_WORDS.contains(&word))
}

/// Whether the step `step` sets the job up rather than running the pull
/// request's own work: one of [`SETUP_STEPS`], or "Set up", "Setup" or
/// "Install" and a known toolchain.
fn setup_step(step: &str) -> bool {
    let step = step.trim().to_lowercase();
    if SETUP_STEPS.iter().any(|setup| step.starts_with(setup)) {
        return true;
    }
    let tool = ["set up ", "setup ", "install "]
        .iter()
        .find_map(|verb| step.strip_prefix(verb));
    tool.and_then(|tool| tool.split_whitespace().next())
        .is_some_and(|tool| SETUP_TOOLCHAINS.contains(&tool))
}

/// The first failure `annotations` name, as `path: title`, in a fixed order
/// whatever order GitHub gives: a test that failed, by its file and its
/// title, at most [`FAILURE_BYTES`]. An annotation with no title names none:
/// its message is log output, which another app may not mask. GitHub
/// Actions' own errors name no test and are left out.
fn failure_of(annotations: &[FailureAnnotation]) -> Option<String> {
    annotations
        .iter()
        .filter_map(|annotation| {
            let path = annotation.path.trim();
            if path.is_empty() || path == ACTIONS_ANNOTATION_PATH {
                return None;
            }
            let title = annotation
                .title
                .as_deref()
                .and_then(|title| title.lines().next())
                .map(str::trim)
                .filter(|title| !title.is_empty())?;
            Some(format!("{path}: {title}"))
        })
        .min()
        .map(|failure| cut(&failure, FAILURE_BYTES).to_string())
}

/// How the finished `job` is kept: none for one still running, or with no
/// name. A failure is the CI's setup when the step that failed sets the job
/// up, or its output or an annotation says GitHub Actions could not set it
/// up; it names the first test its check run annotated as failed.
pub(crate) fn ci_job(job: &FinishedJob) -> Option<CiJob> {
    let name = kept_name(job.name.trim());
    if name.is_empty() {
        return None;
    }
    let failed_step = job
        .failed_step
        .as_deref()
        .map(|step| kept_name(step.trim()).to_string())
        .filter(|step| !step.is_empty());
    let result = match &job.state {
        CheckState::Waiting => return None,
        CheckState::Passed => CiJobResult::Passed,
        CheckState::Failed(conclusion) if conclusion == "cancelled" => CiJobResult::Cancelled,
        CheckState::Failed(_) => {
            let setup_step = failed_step.as_deref().is_some_and(setup_step);
            let setup_error = job
                .detail
                .iter()
                .flat_map(|detail| detail.lines())
                .chain(job.annotations.iter().flat_map(|annotation| {
                    annotation
                        .title
                        .iter()
                        .map(String::as_str)
                        .chain(annotation.message.lines())
                }))
                .any(setup_error);
            if setup_step || setup_error {
                CiJobResult::Setup
            } else {
                CiJobResult::Failed
            }
        }
    };
    let failed_step = failed_step.filter(|_| result.is_red());
    let failure = if result.is_red() {
        failure_of(&job.annotations)
    } else {
        None
    };
    Some(CiJob {
        name: name.to_string(),
        result,
        failed_step,
        failure,
        replaced: false,
    })
}

/// How the finished `jobs` are kept, each name once.
pub(crate) fn ci_jobs(jobs: &[FinishedJob]) -> Vec<CiJob> {
    let mut kept: Vec<CiJob> = Vec::new();
    for job in jobs.iter().filter_map(ci_job) {
        if !kept.iter().any(|other| other.name == job.name) {
            kept.push(job);
        }
    }
    kept
}

// The query.

/// The period, Flow and repository CI health and review quality are asked
/// for.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityQuery {
    period: InsightsQuery,
    repository: Option<String>,
}

impl QualityQuery {
    /// The Runs of `period`, of the repository `repository` names, or of
    /// every repository. It is named as `owner/name` or by its URL, with or
    /// without `https://`, a `.git` ending or a closing slash, in any case:
    /// GitHub names repositories case-insensitively, so it is kept as its
    /// URL in lower case, and Runs are matched in lower case too.
    pub fn new(period: InsightsQuery, repository: Option<String>) -> Result<Self, InsightsError> {
        let repository = repository
            .map(|named| {
                normalized_repository(&named).ok_or(InsightsError::InvalidRepository(named))
            })
            .transpose()?;
        Ok(Self { period, repository })
    }

    pub(crate) fn scope(
        &self,
        actor: &MacroUserIdStr<'static>,
    ) -> Result<QualityScope, InsightsError> {
        Ok(QualityScope {
            runs: self.period.scope(actor)?,
            repository: self.repository.clone(),
        })
    }
}

/// The URL, `https://github.com/owner/name` in lower case, of the
/// repository `named` names, if it names one.
fn normalized_repository(named: &str) -> Option<String> {
    let lower = named.trim().to_lowercase();
    let path = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .unwrap_or(&lower);
    let path = path.strip_prefix("www.").unwrap_or(path);
    let path = path.strip_prefix("github.com/").unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    CodeRepository::parse(&format!("https://github.com/{path}")).map(|repository| repository.url())
}

// What the person reads.

/// What a CI job's failure was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CiCause {
    /// The pull request's own: no other pull request failed the same way.
    Own,
    /// The CI's setup: it failed before its own work ran.
    Setup,
    /// Flaky: the same job failed the same way, in the same test or the
    /// same named step, on two or more pull requests.
    Flaky,
    /// Cancelled, as when a newer push replaced its run.
    Cancelled,
}

/// How many CI failures had each cause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CiCauseCounts {
    /// The pull request's own.
    pub own: u32,
    /// The CI's setup.
    pub setup: u32,
    /// Flaky.
    pub flaky: u32,
    /// Cancelled.
    pub cancelled: u32,
}

impl CiCauseCounts {
    fn add(&mut self, cause: CiCause) {
        let count = match cause {
            CiCause::Own => &mut self.own,
            CiCause::Setup => &mut self.setup,
            CiCause::Flaky => &mut self.flaky,
            CiCause::Cancelled => &mut self.cancelled,
        };
        *count += 1;
    }
}

/// One CI job across the period's waits.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CiJobHealth {
    /// The repository it runs in, by URL.
    pub repository: Option<String>,
    /// Its name.
    pub name: String,
    /// How many times it finished, passed or red; a cancelled run is not
    /// counted.
    pub finished: u32,
    /// How many of those were red.
    pub red: u32,
    /// The share of them that were red, from 0 to 1.
    pub red_rate: f64,
    /// Its failures by cause, cancelled runs included.
    pub causes: CiCauseCounts,
    /// How many pull requests it was red on.
    pub pull_requests_failed: u32,
}

/// A failure of a job, by the test or the named step that failed, on two or
/// more pull requests of the period.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecurringCiFailure {
    /// The repository, by URL.
    pub repository: Option<String>,
    /// The job.
    pub job: String,
    /// The step that failed, when it was read.
    pub step: Option<String>,
    /// The test that failed, as its check run annotated it: `path: name`.
    pub failure: Option<String>,
    /// The CI's setup, or flaky.
    pub cause: CiCause,
    /// How many pull requests it failed on.
    pub pull_requests: u32,
    /// How many times it failed.
    pub failures: u32,
    /// When it last failed.
    pub last_failed_at: DateTime<Utc>,
    /// The newest Runs it failed in.
    pub runs: Vec<RunLink>,
}

/// How the period's pull requests fared in CI.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CiHealth {
    /// Waits on CI that ended: green, red or timed out.
    pub waits: u32,
    /// Those that ended red.
    pub red_waits: u32,
    /// Runs that waited on CI.
    pub runs: u32,
    /// The jobs' failures by cause.
    pub causes: CiCauseCounts,
    /// Each job, the reddest first.
    pub jobs: Vec<CiJobHealth>,
    /// From a pull request's first wait on CI, once it was pushed, to the
    /// end of its first green wait.
    pub first_push_to_green: Option<DurationStats>,
    /// Rounds Runs spent fixing CI: each red wait sends the change to be
    /// fixed.
    pub ci_fix_rounds: u32,
    /// Runs that spent one or more.
    pub runs_with_ci_fixes: u32,
    /// Rounds Runs spent fixing reviewers' Blockers, apart from CI.
    pub review_fix_rounds: u32,
    /// Failures, by test or named step, on two or more pull requests,
    /// newest first.
    pub recurring: Vec<RecurringCiFailure>,
}

/// One reviewer's, or one Agent's, Blockers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlockerCount {
    /// The reviewer's Role, or the Agent.
    pub name: String,
    /// The reviews it made: its reviewer Steps.
    pub reviews: u32,
    /// The Blockers it found.
    pub blockers: u32,
}

/// A Blocker text found in two or more Runs.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecurringBlocker {
    /// The text, as last written.
    pub text: String,
    /// In how many Runs it was found.
    pub runs: u32,
    /// How many times.
    pub blockers: u32,
    /// The reviewers' Roles that found it.
    pub reviewers: Vec<String>,
    /// When its round last started.
    pub last_found_at: DateTime<Utc>,
    /// The newest Runs it was found in.
    pub run_links: Vec<RunLink>,
}

/// How the period's review rounds went.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQuality {
    /// Runs with one or more review rounds.
    pub reviewed_runs: u32,
    /// Their rounds.
    pub rounds: u32,
    /// Rounds per reviewed Run.
    pub rounds_per_run: Option<f64>,
    /// Runs whose first round ended.
    pub first_rounds: u32,
    /// Those whose first round found no Blocker.
    pub clean_first_rounds: u32,
    /// The share of them clean, from 0 to 1.
    pub clean_first_rate: Option<f64>,
    /// Blockers found.
    pub blockers: u32,
    /// Blockers no earlier round of their Run found.
    pub new_blockers: u32,
    /// Blockers an earlier round of their Run found too: not fixed.
    pub repeated_blockers: u32,
    /// Blockers by reviewer Role, most first.
    pub by_reviewer: Vec<BlockerCount>,
    /// Blockers by Agent, most first.
    pub by_agent: Vec<BlockerCount>,
    /// Blocker texts found in two or more Runs, in the most Runs first.
    pub recurring: Vec<RecurringBlocker>,
}

/// What merging cost, and how often it took no person.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryInsight {
    /// Runs that ended.
    pub ended_runs: u32,
    /// Runs that merged their pull request.
    pub merged: u32,
    /// Runs that merged with no person stepping in: no Gate a person
    /// answered, no Step they steered, and no manual end.
    pub autonomous: u32,
    /// The share of ended Runs that did, from 0 to 1.
    pub autonomy_rate: Option<f64>,
    /// Ended Runs with a Gate a person answered.
    pub person_answered: u32,
    /// Ended Runs with a Step their owner steered.
    pub steered: u32,
    /// Runs that ended for a person to take over.
    pub manual: u32,
    /// Runs with usage recorded.
    pub measured_runs: u32,
    /// The tokens every Run's agents used.
    pub tokens: TokenCounts,
    /// What their priced model calls cost, in US dollars.
    pub cost_usd: f64,
    /// Their model calls with no price.
    pub unpriced_calls: u64,
    /// What every Run cost per merged pull request, in US dollars; none
    /// unless every Run's agent sessions recorded their usage and every
    /// call was priced.
    pub cost_per_merged_usd: Option<f64>,
    /// The tokens every Run used per merged pull request; none unless every
    /// Run's agent sessions recorded their usage.
    pub tokens_per_merged: Option<u64>,
}

/// CI health, review quality, cost and autonomy of a person's Runs over a
/// period.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QualityInsights {
    /// The period's first day.
    pub from: NaiveDate,
    /// Its last day.
    pub to: NaiveDate,
    /// The time zone its days are in.
    pub time_zone: String,
    /// The Flow read, or none for every Flow.
    pub flow: Option<String>,
    /// The repository read, by URL, or none for every repository.
    pub repository: Option<String>,
    /// The repositories the period's Runs work on, to pick one from.
    pub repositories: Vec<String>,
    /// The most Runs read: the newest in the period.
    pub run_cap: u16,
    /// Whether the period had more Runs than that.
    pub capped: bool,
    /// How their pull requests fared in CI.
    pub ci: CiHealth,
    /// How their review rounds went.
    pub review: ReviewQuality,
    /// What merging cost, and how often it took no person.
    pub delivery: DeliveryInsight,
}

/// Read the usage of `sessions`, `actor`'s, in reads of at most
/// [`SESSIONS_PER_READ`].
pub(crate) async fn read_usage<U: SessionUsageReader>(
    usage: &U,
    actor: &MacroUserIdStr<'static>,
    sessions: &[Uuid],
) -> Result<Vec<SessionTokens>, U::Err> {
    let mut tokens = Vec::new();
    for chunk in sessions.chunks(SESSIONS_PER_READ) {
        tokens.extend(usage.session_tokens(actor, chunk).await?);
    }
    Ok(tokens)
}

/// Sum up `records` for `query`, with `tokens` the usage of their sessions.
pub(crate) fn summarize_quality(
    query: &QualityQuery,
    records: QualityRecords,
    tokens: &[SessionTokens],
) -> QualityInsights {
    let runs: HashMap<RunId, &QualityRun> =
        records.runs.iter().map(|run| (run.run.id, run)).collect();
    QualityInsights {
        from: query.period.from(),
        to: query.period.to(),
        time_zone: query.period.time_zone().to_string(),
        flow: query.period.flow().map(str::to_string),
        // The repository as the Runs name it, when one does.
        repository: query.repository.as_ref().map(|asked| {
            records
                .repositories
                .iter()
                .find(|named| named.eq_ignore_ascii_case(asked))
                .unwrap_or(asked)
                .clone()
        }),
        repositories: records.repositories.clone(),
        run_cap: INSIGHTS_RUN_CAP,
        capped: records.capped,
        ci: ci_health(&records, &runs),
        review: review_quality(&records, &runs),
        delivery: delivery(&records, tokens),
    }
}

/// The pull request a Run opened, to tell pull requests apart: its page,
/// or the Run itself when it kept none.
fn pull_request_of(runs: &HashMap<RunId, &QualityRun>, run_id: RunId) -> String {
    runs.get(&run_id)
        .and_then(|run| run.pull_request.clone())
        .unwrap_or_else(|| run_id.to_string())
}

fn repository_of(runs: &HashMap<RunId, &QualityRun>, run_id: RunId) -> Option<String> {
    runs.get(&run_id).and_then(|run| run.repository.clone())
}

/// Links to `run_ids`' Runs, newest first, at most [`RUNS_LINKED`].
fn run_links(runs: &HashMap<RunId, &QualityRun>, run_ids: &BTreeSet<RunId>) -> Vec<RunLink> {
    let mut links: Vec<RunLink> = run_ids
        .iter()
        .filter_map(|run_id| runs.get(run_id))
        .map(|run| RunLink::from(&run.run))
        .collect();
    links.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    links.truncate(RUNS_LINKED);
    links
}

fn share(part: u32, whole: u32) -> Option<f64> {
    (whole > 0).then(|| f64::from(part) / f64::from(whole))
}

/// A job, in a repository, failing the same way: in a step, and a test.
type FailureKey = (Option<String>, String, Option<String>, Option<String>);

/// What tells the red `job`'s failure from another pull request's: its
/// step and the test that failed. A setup failure is told by its step. A
/// failure of the job's own work needs a test, or a step named for
/// something narrower than the job's whole work; none otherwise, as for a
/// generic step like "Run tests" or a step that could not be read, which
/// is its pull request's own failure.
fn failure_identity(job: &CiJob) -> Option<(Option<String>, Option<String>)> {
    match job.result {
        CiJobResult::Setup => Some((job.failed_step.clone(), job.failure.clone())),
        CiJobResult::Failed => {
            let named_step = job
                .failed_step
                .as_deref()
                .is_some_and(|step| !generic_step(step));
            (job.failure.is_some() || named_step)
                .then(|| (job.failed_step.clone(), job.failure.clone()))
        }
        CiJobResult::Passed | CiJobResult::Cancelled => None,
    }
}

/// The key `job`'s failure is counted under, in `repository`, if it has an
/// identity.
fn failure_key(repository: &Option<String>, job: &CiJob) -> Option<FailureKey> {
    let (step, failure) = failure_identity(job)?;
    Some((repository.clone(), job.name.clone(), step, failure))
}

/// What one failure's occurrences add up to.
#[derive(Default)]
struct Failures {
    pull_requests: BTreeSet<String>,
    runs: BTreeSet<RunId>,
    count: u32,
    all_setup: bool,
    last: Option<DateTime<Utc>>,
}

fn ci_health(records: &QualityRecords, runs: &HashMap<RunId, &QualityRun>) -> CiHealth {
    // The pull requests each failure happened on, first, to tell a flaky
    // failure from the pull request's own.
    let mut failures: BTreeMap<FailureKey, Failures> = BTreeMap::new();
    for wait in &records.ci_waits {
        let repository = repository_of(runs, wait.run_id);
        for job in &wait.jobs {
            let Some(key) = failure_key(&repository, job) else {
                continue;
            };
            let failed = failures.entry(key).or_insert_with(|| Failures {
                all_setup: true,
                ..Failures::default()
            });
            failed
                .pull_requests
                .insert(pull_request_of(runs, wait.run_id));
            failed.runs.insert(wait.run_id);
            failed.count += 1;
            failed.all_setup &= job.result == CiJobResult::Setup;
            failed.last = failed.last.max(Some(wait.ended_at));
        }
    }
    let cause_of = |repository: &Option<String>, job: &CiJob| match job.result {
        CiJobResult::Passed => None,
        CiJobResult::Cancelled => Some(CiCause::Cancelled),
        CiJobResult::Setup => Some(CiCause::Setup),
        CiJobResult::Failed => {
            let spread = failure_key(repository, job)
                .and_then(|key| failures.get(&key))
                .map_or(0, |failed| failed.pull_requests.len());
            Some(if spread >= FLAKY_PULL_REQUESTS {
                CiCause::Flaky
            } else {
                CiCause::Own
            })
        }
    };

    struct JobTally {
        finished: u32,
        red: u32,
        causes: CiCauseCounts,
        pull_requests: BTreeSet<String>,
    }
    let mut jobs: BTreeMap<(Option<String>, String), JobTally> = BTreeMap::new();
    let mut causes = CiCauseCounts::default();
    let mut red_by_run: BTreeMap<RunId, u32> = BTreeMap::new();
    let mut waited: BTreeSet<RunId> = BTreeSet::new();
    let mut red_waits = 0;
    // Each Run's first wait's start, and its first green wait's end.
    let mut first_wait: HashMap<RunId, (u16, DateTime<Utc>)> = HashMap::new();
    let mut first_green: HashMap<RunId, (u16, DateTime<Utc>)> = HashMap::new();
    for wait in &records.ci_waits {
        waited.insert(wait.run_id);
        let earlier = |kept: &(u16, DateTime<Utc>)| kept.0 < wait.seq;
        if !first_wait.get(&wait.run_id).is_some_and(earlier) {
            first_wait.insert(wait.run_id, (wait.seq, wait.started_at));
        }
        match wait.verdict {
            CiVerdict::Red => {
                red_waits += 1;
                *red_by_run.entry(wait.run_id).or_default() += 1;
            }
            CiVerdict::Green => {
                if !first_green.get(&wait.run_id).is_some_and(earlier) {
                    first_green.insert(wait.run_id, (wait.seq, wait.ended_at));
                }
            }
            CiVerdict::Timeout => {}
        }
        let repository = repository_of(runs, wait.run_id);
        for job in &wait.jobs {
            let tally = jobs
                .entry((repository.clone(), job.name.clone()))
                .or_insert_with(|| JobTally {
                    finished: 0,
                    red: 0,
                    causes: CiCauseCounts::default(),
                    pull_requests: BTreeSet::new(),
                });
            if job.result != CiJobResult::Cancelled {
                tally.finished += 1;
            }
            if job.result.is_red() {
                tally.red += 1;
                tally
                    .pull_requests
                    .insert(pull_request_of(runs, wait.run_id));
            }
            if let Some(cause) = cause_of(&repository, job) {
                tally.causes.add(cause);
                causes.add(cause);
            }
        }
    }

    let mut jobs: Vec<CiJobHealth> = jobs
        .into_iter()
        .map(|((repository, name), tally)| CiJobHealth {
            repository,
            name,
            finished: tally.finished,
            red: tally.red,
            red_rate: share(tally.red, tally.finished).unwrap_or(0.0),
            causes: tally.causes,
            pull_requests_failed: u32::try_from(tally.pull_requests.len()).unwrap_or(u32::MAX),
        })
        .collect();
    jobs.sort_by(|a, b| {
        b.red
            .cmp(&a.red)
            .then_with(|| b.red_rate.total_cmp(&a.red_rate))
            .then_with(|| a.name.cmp(&b.name))
    });
    jobs.truncate(JOBS_LISTED);

    let to_green: Vec<f64> = first_green
        .iter()
        .filter_map(|(run_id, (_, green_at))| {
            let (_, started_at) = first_wait.get(run_id)?;
            Some((*green_at - *started_at).num_milliseconds().max(0) as f64 / 1000.0)
        })
        .collect();

    let mut recurring: Vec<RecurringCiFailure> = failures
        .into_iter()
        .filter(|(_, failed)| failed.pull_requests.len() >= FLAKY_PULL_REQUESTS)
        .filter_map(|((repository, job, step, failure), failed)| {
            Some(RecurringCiFailure {
                repository,
                job,
                step,
                failure,
                cause: if failed.all_setup {
                    CiCause::Setup
                } else {
                    CiCause::Flaky
                },
                pull_requests: u32::try_from(failed.pull_requests.len()).unwrap_or(u32::MAX),
                failures: failed.count,
                last_failed_at: failed.last?,
                runs: run_links(runs, &failed.runs),
            })
        })
        .collect();
    recurring.sort_by(|a, b| {
        b.last_failed_at
            .cmp(&a.last_failed_at)
            .then_with(|| a.job.cmp(&b.job))
    });
    recurring.truncate(RECURRING_LISTED);

    let review_fix_rounds = records
        .review_rounds
        .iter()
        .filter(|round| round.outcome == Some(ReviewRoundOutcome::Changes))
        .count();

    CiHealth {
        waits: u32::try_from(records.ci_waits.len()).unwrap_or(u32::MAX),
        red_waits,
        runs: u32::try_from(waited.len()).unwrap_or(u32::MAX),
        causes,
        jobs,
        first_push_to_green: duration_stats(to_green),
        ci_fix_rounds: red_by_run.values().sum(),
        runs_with_ci_fixes: u32::try_from(red_by_run.len()).unwrap_or(u32::MAX),
        review_fix_rounds: u32::try_from(review_fix_rounds).unwrap_or(u32::MAX),
        recurring,
    }
}

/// A Blocker's text as compared with others: in lower case, its spaces
/// collapsed and its closing stop dropped.
fn blocker_text(problem: &str) -> String {
    problem
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .trim_end_matches(['.', ':', ';', '!'])
        .to_string()
}

fn by_count(counts: BTreeMap<String, (u32, u32)>) -> Vec<BlockerCount> {
    let mut counts: Vec<BlockerCount> = counts
        .into_iter()
        .map(|(name, (reviews, blockers))| BlockerCount {
            name,
            reviews,
            blockers,
        })
        .collect();
    counts.sort_by(|a, b| {
        b.blockers
            .cmp(&a.blockers)
            .then_with(|| a.name.cmp(&b.name))
    });
    counts
}

/// A reviewer Step's Role and Agent, when it recorded them.
type ReviewerNames<'a> = (Option<&'a str>, Option<&'a str>);

fn review_quality(records: &QualityRecords, runs: &HashMap<RunId, &QualityRun>) -> ReviewQuality {
    // Rounds, and each Run's first.
    let mut first_round: BTreeMap<RunId, (u16, Option<ReviewRoundOutcome>)> = BTreeMap::new();
    let mut round_started: HashMap<(RunId, u16), DateTime<Utc>> = HashMap::new();
    for round in &records.review_rounds {
        round_started.insert((round.run_id, round.seq), round.started_at);
        let first = first_round
            .entry(round.run_id)
            .or_insert((round.seq, round.outcome));
        if round.seq < first.0 {
            *first = (round.seq, round.outcome);
        }
    }
    let first_rounds = first_round
        .values()
        .filter(|(_, outcome)| outcome.is_some())
        .count();
    let clean_first_rounds = first_round
        .values()
        .filter(|(_, outcome)| *outcome == Some(ReviewRoundOutcome::Pass))
        .count();
    let reviewed_runs = u32::try_from(first_round.len()).unwrap_or(u32::MAX);
    let rounds = u32::try_from(records.review_rounds.len()).unwrap_or(u32::MAX);
    let first_rounds = u32::try_from(first_rounds).unwrap_or(u32::MAX);
    let clean_first_rounds = u32::try_from(clean_first_rounds).unwrap_or(u32::MAX);

    // Reviews by reviewer Role and by Agent.
    let mut by_reviewer: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut by_agent: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    let mut reviewer_of: HashMap<(RunId, u16), ReviewerNames<'_>> = HashMap::new();
    for reviewer in &records.reviewers {
        reviewer_of.insert(
            (reviewer.run_id, reviewer.seq),
            (reviewer.role.as_deref(), reviewer.agent.as_deref()),
        );
        if let Some(role) = &reviewer.role {
            by_reviewer.entry(role.clone()).or_default().0 += 1;
        }
        if let Some(agent) = &reviewer.agent {
            by_agent.entry(agent.clone()).or_default().0 += 1;
        }
    }

    // Blockers, in their rounds' order: one an earlier round of its Run
    // found too is repeated.
    let mut ordered: Vec<_> = records.blockers.iter().collect();
    ordered.sort_by_key(|blocker| (blocker.run_id, blocker.round_seq, blocker.seq));
    let mut found_before: HashMap<RunId, BTreeMap<String, u16>> = HashMap::new();
    let mut new_blockers = 0;
    let mut repeated_blockers = 0;
    struct Recurring {
        text: String,
        runs: BTreeSet<RunId>,
        count: u32,
        reviewers: BTreeSet<String>,
        last: DateTime<Utc>,
    }
    let mut texts: BTreeMap<String, Recurring> = BTreeMap::new();
    for blocker in ordered {
        let text = blocker_text(&blocker.problem);
        let earlier = found_before.entry(blocker.run_id).or_default();
        let repeated = earlier
            .get(&text)
            .is_some_and(|round| *round < blocker.round_seq);
        if repeated {
            repeated_blockers += 1;
        } else {
            new_blockers += 1;
            earlier.entry(text.clone()).or_insert(blocker.round_seq);
        }
        let (role, agent) = reviewer_of
            .get(&(blocker.run_id, blocker.seq))
            .copied()
            .unwrap_or_default();
        if let Some(role) = role {
            by_reviewer.entry(role.to_string()).or_default().1 += 1;
        }
        if let Some(agent) = agent {
            by_agent.entry(agent.to_string()).or_default().1 += 1;
        }
        let found_at = round_started
            .get(&(blocker.run_id, blocker.round_seq))
            .copied()
            .or_else(|| runs.get(&blocker.run_id).map(|run| run.run.created_at))
            .unwrap_or_default();
        let recurring = texts.entry(text).or_insert_with(|| Recurring {
            text: blocker.problem.clone(),
            runs: BTreeSet::new(),
            count: 0,
            reviewers: BTreeSet::new(),
            last: found_at,
        });
        recurring.runs.insert(blocker.run_id);
        recurring.count += 1;
        if let Some(role) = role {
            recurring.reviewers.insert(role.to_string());
        }
        if found_at >= recurring.last {
            recurring.last = found_at;
            recurring.text = blocker.problem.clone();
        }
    }
    let mut recurring: Vec<RecurringBlocker> = texts
        .into_values()
        .filter(|text| text.runs.len() >= 2)
        .map(|text| RecurringBlocker {
            text: text.text,
            runs: u32::try_from(text.runs.len()).unwrap_or(u32::MAX),
            blockers: text.count,
            reviewers: text.reviewers.into_iter().collect(),
            last_found_at: text.last,
            run_links: run_links(runs, &text.runs),
        })
        .collect();
    recurring.sort_by(|a, b| {
        b.runs
            .cmp(&a.runs)
            .then_with(|| b.last_found_at.cmp(&a.last_found_at))
    });
    recurring.truncate(RECURRING_LISTED);

    ReviewQuality {
        reviewed_runs,
        rounds,
        rounds_per_run: (reviewed_runs > 0).then(|| f64::from(rounds) / f64::from(reviewed_runs)),
        first_rounds,
        clean_first_rounds,
        clean_first_rate: share(clean_first_rounds, first_rounds),
        blockers: new_blockers + repeated_blockers,
        new_blockers,
        repeated_blockers,
        by_reviewer: by_count(by_reviewer),
        by_agent: by_count(by_agent),
        recurring,
    }
}

fn delivery(records: &QualityRecords, tokens: &[SessionTokens]) -> DeliveryInsight {
    let by_session: HashMap<Uuid, &SessionTokens> = tokens
        .iter()
        .map(|tokens| (tokens.session_id, tokens))
        .collect();
    let mut ended_runs = 0;
    let mut merged = 0;
    let mut autonomous = 0;
    let mut person_answered = 0;
    let mut steered = 0;
    let mut manual = 0;
    // The bots one of whose sessions recorded usage: their runtime records
    // it, so a session of theirs that recorded none made no model call (it
    // failed before its first) and cost nothing.
    let measured_bots: HashSet<Uuid> = records
        .runs
        .iter()
        .flat_map(|run| &run.sessions)
        .filter(|session| by_session.contains_key(&session.id))
        .filter_map(|session| session.bot)
        .collect();
    let mut measured_runs = 0;
    // Runs with an agent session that recorded no usage on a runtime that
    // records none: one on someone's own login (Codex or Claude Code
    // through macrod), so what those Runs spent is not known. A session
    // whose bot is not known counts too.
    let mut unmeasured_runs = 0;
    let mut used = TokenCounts::default();
    let mut cost_usd = 0.0;
    let mut unpriced_calls = 0;
    for run in &records.runs {
        let outcome = run.run.end.map(|(outcome, _)| outcome);
        let ended = outcome.is_some();
        let is_manual = outcome == Some(RunOutcome::Manual);
        if ended {
            ended_runs += 1;
            person_answered += u32::from(run.person_answered);
            steered += u32::from(run.steered);
            manual += u32::from(is_manual);
        }
        if run.merged {
            merged += 1;
            if ended && !run.person_answered && !run.steered && !is_manual {
                autonomous += 1;
            }
        }
        let mut measured = false;
        let mut unmeasured = false;
        for session in &run.sessions {
            let Some(session) = by_session.get(&session.id) else {
                unmeasured |= !session.bot.is_some_and(|bot| measured_bots.contains(&bot));
                continue;
            };
            measured = true;
            used.input += session.input;
            used.output += session.output;
            used.cache_read += session.cache_read;
            used.cache_write += session.cache_write;
            cost_usd += session.cost_usd;
            unpriced_calls += session.unpriced_calls;
        }
        measured_runs += u32::from(measured);
        unmeasured_runs += u32::from(unmeasured);
    }
    let total_tokens = used.input + used.output + used.cache_read + used.cache_write;
    // A figure per merged pull request is every Run's spend over the
    // merges: only known when every Run's agent sessions recorded theirs.
    let whole = measured_runs > 0 && unmeasured_runs == 0;
    let priced = whole && unpriced_calls == 0;
    DeliveryInsight {
        ended_runs,
        merged,
        autonomous,
        autonomy_rate: share(autonomous, ended_runs),
        person_answered,
        steered,
        manual,
        measured_runs,
        tokens: used,
        cost_usd,
        unpriced_calls,
        cost_per_merged_usd: (merged > 0 && priced).then(|| cost_usd / f64::from(merged)),
        tokens_per_merged: (merged > 0 && whole).then(|| total_tokens / u64::from(merged)),
    }
}
