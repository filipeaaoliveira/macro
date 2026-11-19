//! The Steps that take a Run's branch to a merged pull request, on GitHub as
//! the Run's owner: `publish` opens the pull request, `ci` waits for its
//! checks, `ready_to_merge` shows a person the commit that passed them, and
//! `merge` merges that commit once they agree.
//!
//! Each reads GitHub before it changes anything, so a Temporal retry, or a
//! Run sent back to a Step it took before, finds the pull request it opened
//! or merged instead of opening or merging another.
//!
//! `publish`, `ready_to_merge` and `merge` check the Control Files the
//! commit they act on changed against the base (`control_files`), and keep
//! them on the Run: one its request does not ask for makes it Tampered.
//! `merge` merges a Tampered Run only when the answer that agreed to it
//! approved those files; otherwise it sends the Run back through `ci` to
//! the merge Gate, which shows them.
//!
//! While a Main Hold holds the Run's base, `merge` merges nothing: it waits
//! for the hold to clear, then ends `held`, which goes back to the merge Gate.
//!
//! Once its pull request merged, the Run's branch is deleted, as GitHub's
//! "delete branch after merge" does; a Run that ends unmerged keeps it.
//!
//! On a Flow that routes them, Runs merge to a base one after the other, in
//! the order they first reached their merge (the merge queue), and never
//! merge a branch that lacks a commit of its base: `ready_to_merge` and
//! `merge` end `behind` instead, and the `update` Step merges the base into
//! the branch ([`PullRequestSteps::update_branch`]), after which its checks
//! and CI run again and the merge Gate asks again. `merge` ends `queued`
//! when the Runs ahead of it are still there after [`QUEUE_WAIT`].

#[cfg(test)]
mod test;

use std::time::Duration;

use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::{MainHold, MainHoldRepository};
use factory_records::domain::merge_queue::{BranchUpdate, MergeQueueRepository, QUEUE_PLACE_KEPT};
use factory_records::domain::models::{ControlFileChanges, Outcome, Repository};
use factory_records::domain::ports::RunRepository;
use github::domain::models::MacroTaskId;

use crate::domain::checks::CHECKS_FILE;
use crate::domain::control_files::{ControlFiles, ControlFilesError};
use crate::domain::definition::DEFINITION_FILE_PATTERNS;
use crate::domain::main_hold::{MainWatch, held_note};
use crate::domain::models::{
    BranchDeletion, BranchUpdating, CheckState, CheckSuite, CodeRepository, HeadChecks, Merging,
    OpenPullRequest, Opening, PullRequest, PullRequestState, RequiredCheck, RequiredChecks,
    StepInput, StepReport, run_branch,
};
use crate::domain::ports::{MainWatches, PullRequests, Retryable};
use crate::domain::setup::SETUP_FILE;
use crate::domain::verification::{VERIFY_FILE, VerifyFile, VerifyFileError};

/// How long `ci` waits for a pull request's checks to finish: short of a
/// Step attempt's 30 minutes, so the Step ends `timeout` rather than failing.
pub const CI_WAIT: Duration = Duration::from_secs(25 * 60);

/// How long `ci` waits between two reads of the checks.
const CI_POLL: Duration = Duration::from_secs(20);

/// How long `merge` waits for a Main Hold on its base to clear: short of a
/// Step attempt's 30 minutes, so the Step ends `held` rather than failing.
pub const HOLD_WAIT: Duration = Duration::from_secs(25 * 60);

/// How long `merge` waits between two reads of its base's hold.
const HOLD_POLL: Duration = Duration::from_secs(30);

/// How long `merge` waits for the Runs ahead of it in its base's merge
/// queue, from its start, the wait for a Main Hold included: short of a
/// Step attempt's 30 minutes, so the Step ends `queued` rather than failing.
pub const QUEUE_WAIT: Duration = Duration::from_secs(25 * 60);

/// How long `merge` waits between two reads of its base's merge queue.
const QUEUE_POLL: Duration = Duration::from_secs(30);

/// How long an update waits for GitHub's merge of the base into the Run's
/// branch to show on the branch, before its attempt is left to a retry.
const UPDATE_SHOWS_WITHIN: Duration = Duration::from_secs(120);

/// How long an update waits between two reads of the Run's branch.
const UPDATE_POLL: Duration = Duration::from_secs(3);

/// The longest pull request title the Steps write.
const MAX_TITLE_CHARS: usize = 72;

/// What `ci` tells the Run's owner when their token may not read GitHub
/// Actions runs.
const ACTIONS_UNREAD_NOTE: &str = "\n\nThe GitHub account linked to Macro may not read this repository's \
     Actions runs, so a run that waits was seen only through an empty Actions suite.";

/// The most characters GitHub keeps in a pull request's description.
const MAX_BODY_CHARS: usize = 65_536;

/// How long a check suite that holds no run, of an app with no run waiting
/// on the head, is waited for: one that adds its runs after it made the suite
/// fills it within this; an app that checks nothing never does.
pub(crate) const EMPTY_SUITE_GRACE: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

/// Why a pull request Step could not be done.
#[derive(Debug, thiserror::Error)]
pub enum PullRequestStepError {
    /// The Run works on no repository.
    #[error("the run works on no repository")]
    NoRepository,
    /// The Run's repository URL does not name a repository.
    #[error("the run's repository {0} is not a https://github.com/owner/name URL")]
    InvalidRepository(String),
    /// GitHub opened the pull request, yet does not list it.
    #[error("github opened the pull request but does not list it yet")]
    NotListed,
    /// The Run's branch has no pull request.
    #[error("the run's branch has no pull request")]
    NoPullRequest,
    /// The Run asked no person to merge a commit.
    #[error("the run asked no one to merge a commit")]
    NoMergeHead,
    /// The Run's base branch is not on GitHub.
    #[error("the run's base branch is not on github")]
    NoBase,
    /// Someone pushed to the Run's branch while it was updated with its
    /// base, or GitHub's merge of the base has not shown on it yet: a retry
    /// reads the branch again.
    #[error("the run's branch moved while it was updated: {0}")]
    BranchMoving(String),
    /// GitHub declined to merge the Run's base into its branch for a reason
    /// other than a conflict, as for a token that lacks a scope.
    #[error("github declined to merge the base into the run's branch: {0}")]
    UpdateDeclined(String),
    /// GitHub could not be asked.
    #[error("github failed: {report:?}")]
    Github {
        /// Why.
        report: rootcause::Report,
        /// Whether asking again may succeed.
        retryable: bool,
    },
    /// The Run's records could not be read or written.
    #[error("the run's records failed: {0:?}")]
    Records(rootcause::Report),
    /// The Run's owner keeps a Control File pattern that does not read.
    #[error(transparent)]
    ControlFiles(#[from] ControlFilesError),
    /// The watch of the Run's base branch could not be started.
    #[error("the watch of the base branch failed: {report:?}")]
    Watch {
        /// Why.
        report: rootcause::Report,
        /// Whether starting it again may succeed.
        retryable: bool,
    },
}

impl PullRequestStepError {
    pub(crate) fn github(
        error: impl std::error::Error + Retryable + Send + Sync + 'static,
    ) -> Self {
        Self::Github {
            retryable: error.retryable(),
            report: rootcause::Report::new(error).into_dynamic(),
        }
    }

    pub(crate) fn records(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Records(rootcause::Report::new(error).into_dynamic())
    }

    fn watch(error: impl std::error::Error + Retryable + Send + Sync + 'static) -> Self {
        Self::Watch {
            retryable: error.retryable(),
            report: rootcause::Report::new(error).into_dynamic(),
        }
    }
}

impl Retryable for PullRequestStepError {
    /// GitHub may list a pull request it just opened a moment later, and a
    /// failed read or write of the records is the database's.
    fn retryable(&self) -> bool {
        match self {
            Self::NotListed | Self::Records(_) | Self::BranchMoving(_) => true,
            Self::Github { retryable, .. } | Self::Watch { retryable, .. } => *retryable,
            Self::NoRepository
            | Self::InvalidRepository(_)
            | Self::NoPullRequest
            | Self::NoMergeHead
            | Self::NoBase
            | Self::UpdateDeclined(_)
            | Self::ControlFiles(_) => false,
        }
    }
}

/// Who merged a Run's pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Merger {
    /// The Run's `merge` Step.
    Run,
    /// Someone outside the Run, as a person on GitHub; or the Run, on a
    /// retry whose merge GitHub took though its answer was lost.
    Outside,
}

/// How long deleting a merged Run's branch may take, its reads included: a
/// slow GitHub keeps the branch rather than the merge Step's time.
const BRANCH_DELETION_WAIT: Duration = Duration::from_secs(60);

/// What a pull request's checks on its head say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// A check has not finished, a required one has not appeared, a list
    /// was longer than was read, or none was reported yet.
    Pending,
    /// Every check that counts passed: the suites' ids, the statuses'
    /// contexts and the required checks that did, so two reads can tell
    /// whether a check appeared between them.
    Green(Vec<String>),
    /// These checks failed, one line each for a person and the CI fix.
    Red(Vec<String>),
    /// The pull request was closed without merging.
    Closed,
}

/// Whether `check` appeared on the head and passed: as a run of the app it
/// names, or, when it names none, as any app's run or a commit status.
pub(crate) fn required_passed(check: &RequiredCheck, checks: &HeadChecks) -> bool {
    let run = checks.runs.iter().any(|run| {
        run.name == check.name
            && run.state == CheckState::Passed
            && check.app_id.is_none_or(|app_id| run.app_id == Some(app_id))
    });
    let status = check.app_id.is_none()
        && checks
            .statuses
            .iter()
            .any(|status| status.context == check.name && status.state == CheckState::Passed);
    run || status
}

/// What the checks of `pull_request` say at `now`, given the checks its base
/// `required`, as far as they could be read.
///
/// Any failure is red. Otherwise every suite that holds a run, and every
/// commit status, must have passed, and every required check that could be
/// named must have appeared and passed. A suite that holds no run yet holds
/// CI back unless:
/// - the required checks were read in full, name at least one check, and all
///   passed: the owner's rule is then met, and another app's empty suite does
///   not matter;
/// - or [`EMPTY_SUITE_GRACE`] passed since GitHub made it: an app that checks
///   nothing on the commit never fills it.
///
/// Short of the owner's rule, an app with a run on the head not finished, or
/// runs not all read, as a GitHub Actions run a concurrency group holds,
/// holds CI until it times out, whether or not its suite exists yet.
/// Lists longer than were read, or nothing reported, wait. A merged pull
/// request is green.
fn verdict(
    pull_request: &PullRequest,
    checks: &HeadChecks,
    required: Option<&RequiredChecks>,
    now: DateTime<Utc>,
) -> Verdict {
    match pull_request.state {
        PullRequestState::Merged => Verdict::Green(Vec::new()),
        PullRequestState::Closed => Verdict::Closed,
        PullRequestState::Open => checks_verdict(checks, required, now),
    }
}

/// What the checks on a commit say at `now`, given the checks its base
/// `required`, as [`verdict`] reads an open pull request's head.
fn checks_verdict(
    checks: &HeadChecks,
    required: Option<&RequiredChecks>,
    now: DateTime<Utc>,
) -> Verdict {
    // A failure anywhere is red at once; the failed runs name what failed
    // in a suite.
    let mut failed = Vec::new();
    for suite in &checks.suites {
        if let CheckState::Failed(conclusion) = &suite.state {
            failed.push(format!("- {} checks ({conclusion})", suite.app));
        }
    }
    for run in &checks.runs {
        if let CheckState::Failed(conclusion) = &run.state {
            let url = run.url.as_deref().unwrap_or("no page");
            failed.push(format!("  - {} ({conclusion}): {url}", run.name));
        }
    }
    for status in &checks.statuses {
        if let CheckState::Failed(state) = &status.state {
            let url = status.url.as_deref().unwrap_or("no page");
            failed.push(format!("- {} ({state}): {url}", status.context));
        }
    }
    if !failed.is_empty() {
        return Verdict::Red(failed);
    }
    if checks.truncated {
        return Verdict::Pending;
    }

    let named: &[RequiredCheck] = required.map_or(&[], |required| &required.checks);
    if !named.iter().all(|check| required_passed(check, checks)) {
        return Verdict::Pending;
    }
    // Only a set read in full may stand for every check that matters.
    let required_met = required.is_some_and(|required| required.complete) && !named.is_empty();

    let counted: Vec<&CheckSuite> = checks
        .suites
        .iter()
        .filter(|suite| {
            if suite.check_runs > 0 {
                return true;
            }
            if required_met {
                return false;
            }
            suite
                .created_at
                .is_none_or(|created_at| now - created_at < EMPTY_SUITE_GRACE)
        })
        .collect();
    // A run not finished, or runs not all read, hold CI, whether or not the
    // app made a suite for them yet.
    if !required_met && !checks.held_apps.is_empty() {
        return Verdict::Pending;
    }
    let finished = counted
        .iter()
        .all(|suite| suite.state == CheckState::Passed)
        && checks
            .statuses
            .iter()
            .all(|status| status.state == CheckState::Passed);
    let reported = !counted.is_empty() || !checks.statuses.is_empty() || required_met;
    if !reported || !finished {
        return Verdict::Pending;
    }

    let mut seen: Vec<String> = counted.iter().map(|suite| suite.id.to_string()).collect();
    seen.extend(checks.statuses.iter().map(|status| status.context.clone()));
    seen.extend(named.iter().map(|check| check.name.clone()));
    seen.sort();
    Verdict::Green(seen)
}

fn outcome(name: &str) -> Outcome {
    Outcome::try_from(name.to_string()).unwrap_or_else(|_| Outcome::failed())
}

fn report(outcome_name: &str, output: String) -> StepReport {
    StepReport {
        outcome: outcome(outcome_name),
        output,
    }
}

/// A pull request's first line for a person: its number and page.
fn named(pull_request: &PullRequest) -> String {
    format!(
        "Pull request #{}: {}",
        pull_request.number, pull_request.url
    )
}

/// What a person reads of the Control Files a Run's branch changed.
/// The outcome of `ready_to_merge` and `merge` for a head that lacks a
/// commit of its base.
pub(crate) const BEHIND: &str = "behind";

/// The outcome of `merge` when Runs ahead of it in its base's merge queue
/// have not merged within [`QUEUE_WAIT`].
pub(crate) const QUEUED: &str = "queued";

/// `behind`, saying why.
fn behind_report(pull_request: &PullRequest, recorded: &Repository, base_head: &str) -> StepReport {
    report(
        BEHIND,
        format!(
            "{}\n\nIt was not merged: {} moved to {base_head}, which its head does not hold. \
             The base is merged into the branch, its checks and CI run again, and the merge \
             Gate asks again.",
            named(pull_request),
            recorded.base_branch
        ),
    )
}

/// What merging the Run's base into its branch came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Update {
    /// GitHub merged the base in, or the branch held it already: the update
    /// kept, with the branch's head it made.
    Merged(BranchUpdate),
    /// GitHub could not merge them: the update kept, for the agent to
    /// resolve.
    Conflict(BranchUpdate),
    /// The pull request was merged or closed meanwhile: the Step's end.
    Ended(StepReport),
}

/// What asking GitHub for an update came to: the update to keep, or the
/// Step's end.
enum Asked {
    Update(BranchUpdate),
    Ended(StepReport),
}

/// Where a kept update leaves the Step without waiting on GitHub: a
/// conflict, for the agent, or a merge whose head is known; none while
/// GitHub's merge has not shown yet.
fn settled(update: &BranchUpdate) -> Option<Update> {
    if update.conflict {
        return Some(Update::Conflict(update.clone()));
    }
    update
        .updated_head
        .is_some()
        .then(|| Update::Merged(update.clone()))
}

fn changes_note(changes: &ControlFileChanges) -> String {
    if changes.unlisted {
        return "\n\nGitHub could not list this branch's changes in full, so it may change Control \
             Files: files that decide whether its checks pass. It merges only once a person \
             approves that."
            .to_string();
    }

    let mut note = String::new();
    if !changes.tampered.is_empty() {
        let files: Vec<String> = changes
            .tampered
            .iter()
            .map(|file| format!("- `{file}`"))
            .collect();
        note.push_str(&format!(
            "\n\nThis branch changes Control Files its request does not ask for, files that \
             decide whether its checks pass:\n\n{}\n\nIt merges only once a person approves them.",
            files.join("\n")
        ));
    }
    if !changes.named.is_empty() {
        let files: Vec<String> = changes
            .named
            .iter()
            .map(|file| format!("- `{file}`"))
            .collect();
        note.push_str(&format!(
            "\n\nIt also changes Control Files named by the request:\n\n{}",
            files.join("\n")
        ));
    }
    note
}

/// The title of the Run's pull request: the request's first line, cut to
/// [`MAX_TITLE_CHARS`].
fn title(request: &str) -> String {
    let first = request
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Change from a Macro factory run");
    if first.chars().count() <= MAX_TITLE_CHARS {
        return first.to_string();
    }
    let cut: String = first.chars().take(MAX_TITLE_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The description of the Run's pull request: the request, then the task it
/// works on, written so GitHub's webhook links the pull request to the task.
/// A request too long for GitHub's [`MAX_BODY_CHARS`] is cut, never the
/// lines after it.
fn body(input: &StepInput<'_>) -> String {
    const CUT: &str = "\n\n(The rest of the request is left out.)";

    let mut footer = format!("\n\nOpened by a Macro factory run ({}).", input.key.run_id);
    if let Some(task_id) = input.task_id {
        footer.push_str(&format!(
            "\n\nTask: {}",
            MacroTaskId::from_uuid(&task_id).to_task_id_string()
        ));
    }

    let room = MAX_BODY_CHARS - footer.chars().count();
    let request = if input.request.chars().count() <= room {
        input.request.to_string()
    } else {
        let kept: String = input
            .request
            .chars()
            .take(room - CUT.chars().count())
            .collect();
        format!("{kept}{CUT}")
    };
    format!("{request}{footer}")
}

/// The pull request Steps, on GitHub through `github`, keeping the commit a
/// person is asked to merge and the merges they make in the Run's records,
/// and starting the watch of the base branch a Run merges to.
#[derive(Debug, Clone)]
pub struct PullRequestSteps<G, R, M> {
    github: G,
    repo: R,
    watches: M,
}

impl<G, R, M> PullRequestSteps<G, R, M>
where
    G: PullRequests,
    R: RunRepository + MainHoldRepository + MergeQueueRepository,
    M: MainWatches,
{
    /// The Steps, over `github`, the Run records `repo` and the base
    /// branches' `watches`.
    pub fn new(github: G, repo: R, watches: M) -> Self {
        Self {
            github,
            repo,
            watches,
        }
    }

    /// The Run's records.
    pub(crate) fn records(&self) -> &R {
        &self.repo
    }

    /// The text of [`CHECKS_FILE`] on the Run's base branch, which the
    /// agent's branch cannot change; none when the base declares no checks.
    pub(crate) async fn declared_checks(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Option<String>, PullRequestStepError> {
        let (repository, recorded) = Self::repository(input)?;
        self.github
            .file_text(input.owner, &repository, &recorded.base_branch, CHECKS_FILE)
            .await
            .map_err(PullRequestStepError::github)
    }

    /// The text of [`SETUP_FILE`] on the Run's base branch, which the
    /// agent's branch cannot change; none when the base declares no setup.
    pub(crate) async fn declared_setup(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Option<String>, PullRequestStepError> {
        let (repository, recorded) = Self::repository(input)?;
        self.github
            .file_text(input.owner, &repository, &recorded.base_branch, SETUP_FILE)
            .await
            .map_err(PullRequestStepError::github)
    }

    /// The commit at the head of the Run's branch on GitHub; none when it
    /// is not there.
    pub(crate) async fn branch_head(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Option<String>, PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;
        self.github
            .branch_head(input.owner, &repository, &run_branch(input.key.run_id))
            .await
            .map_err(PullRequestStepError::github)
    }

    /// The paths the Run's branch changed against its base; none when
    /// GitHub cannot list them all.
    pub(crate) async fn branch_changes(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Option<Vec<String>>, PullRequestStepError> {
        let (repository, recorded) = Self::repository(input)?;
        let branch = run_branch(input.key.run_id);
        self.github
            .changed_paths(input.owner, &repository, &recorded.base_branch, &branch)
            .await
            .map_err(PullRequestStepError::github)
    }

    /// The Control Files `head` changed against the Run's base, by the
    /// patterns the Run keeps from its creation, or the factory's default
    /// for a Run that keeps none: kept on the Run, replacing what was kept
    /// before.
    async fn check_control_files(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        recorded: &Repository,
        head: &str,
    ) -> Result<Option<ControlFileChanges>, PullRequestStepError> {
        let control_files = match input.control_file_patterns {
            Some(patterns) => ControlFiles::parse(patterns)?,
            None => ControlFiles::parse(&ControlFiles::default_patterns())?,
        };

        let changes = if control_files.is_empty() {
            None
        } else {
            let changed = self
                .github
                .changed_paths(input.owner, repository, &recorded.base_branch, head)
                .await
                .map_err(PullRequestStepError::github)?;
            // On its team's Definition repository, a change to the
            // Definition's own files is never one the request asks for: it
            // changes the rules the factory merges under.
            let on_definition = input.definition_repo.zip(input.repository).is_some_and(
                |(definition, repository)| definition.eq_ignore_ascii_case(&repository.url),
            );
            let guarded = if on_definition {
                let patterns = DEFINITION_FILE_PATTERNS.map(str::to_string);
                Some(ControlFiles::parse(&patterns)?)
            } else {
                None
            };
            control_files.changes_guarding(
                changed.as_deref(),
                input.request,
                input.created_in_person,
                guarded.as_ref(),
            )
        };
        self.repo
            .set_control_file_changes(input.key.run_id, changes.as_ref())
            .await
            .map_err(PullRequestStepError::records)?;
        Ok(changes)
    }

    /// What a person reads of the Control Files the Run last kept. A pull
    /// request merged outside the Run is not compared again: GitHub's
    /// comparison of a merged head with its base lists nothing, which would
    /// clear them.
    async fn kept_note(&self, input: &StepInput<'_>) -> Result<String, PullRequestStepError> {
        let kept = self
            .repo
            .get_run(input.key.run_id)
            .await
            .map_err(PullRequestStepError::records)?
            .and_then(|run| run.control_files);
        Ok(kept.as_ref().map(changes_note).unwrap_or_default())
    }

    /// A pull request merged outside the Run: `merged`, naming the Control
    /// Files the Run last kept. The merge is recorded as the Run's, and its
    /// base `recorded` watched, as for a merge the Run made, whichever Step
    /// found it.
    async fn merged_outside(
        &self,
        input: &StepInput<'_>,
        recorded: &Repository,
        pull_request: &PullRequest,
    ) -> Result<StepReport, PullRequestStepError> {
        self.merged(input, recorded, pull_request).await?;
        self.delete_merged_branch(input, &pull_request.head_sha, Merger::Outside)
            .await;
        let note = self.kept_note(input).await?;
        Ok(report("merged", format!("{}{note}", named(pull_request))))
    }

    /// Delete the Run's branch once its pull request merged at
    /// `merged_head`, as GitHub's "delete branch after merge" does. It is
    /// best effort: the pull request is merged either way, so a failure is
    /// logged, never the Step's.
    ///
    /// Only the Run's own branch is deleted, and only while it is at the
    /// commit merged, so one pushed to since is kept, and while no open
    /// pull request is based on it, as deleting it through the API would
    /// close that pull request, where GitHub's own deletion moves it to the
    /// base. GitHub deletes a branch on no condition, so both checks race
    /// the deletion: a push, or a pull request based on the branch, made
    /// between the reads and the deletion is lost with it, or closed.
    ///
    /// A pull request a person merged leaves its branch to GitHub when the
    /// repository deletes merged branches itself, or when the repository
    /// can no longer be read.
    ///
    /// It takes at most [`BRANCH_DELETION_WAIT`], all its reads included.
    #[tracing::instrument(
        name = "factory.delete_run_branch",
        skip_all,
        fields(run_id = %input.key.run_id)
    )]
    async fn delete_merged_branch(&self, input: &StepInput<'_>, merged_head: &str, merger: Merger) {
        let branch = run_branch(input.key.run_id);
        let deleting = self.delete_branch_at(input, &branch, merged_head, merger);
        match tokio::time::timeout(BRANCH_DELETION_WAIT, deleting).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(?error, %branch, "the merged run's branch was not deleted");
            }
            Err(_) => {
                tracing::warn!(%branch, "the merged run's branch was not deleted in time");
            }
        }
    }

    /// [`Self::delete_merged_branch`], with its failure.
    async fn delete_branch_at(
        &self,
        input: &StepInput<'_>,
        branch: &str,
        merged_head: &str,
        merger: Merger,
    ) -> Result<(), PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;

        if merger == Merger::Outside {
            let access = self
                .github
                .repository_access(input.owner, &repository)
                .await
                .map_err(PullRequestStepError::github)?;
            let Some(access) = access else {
                tracing::info!(%branch, "kept the merged run's branch: no access to its repository");
                return Ok(());
            };
            if access.deletes_merged_branches {
                return Ok(());
            }
        }

        // A branch already gone has nothing to delete; one that moved since
        // the merge holds commits no one merged.
        let head = self
            .github
            .branch_head(input.owner, &repository, branch)
            .await
            .map_err(PullRequestStepError::github)?;
        if head.as_deref() != Some(merged_head) {
            if head.is_some() {
                tracing::info!(%branch, "kept the merged run's branch: it moved since");
            }
            return Ok(());
        }

        let based_on = self
            .github
            .has_open_pull_request_onto(input.owner, &repository, branch)
            .await
            .map_err(PullRequestStepError::github)?;
        if based_on {
            tracing::info!(%branch, "kept the merged run's branch: a pull request is based on it");
            return Ok(());
        }

        let deletion = self
            .github
            .delete_run_branch(input.owner, &repository, input.key.run_id)
            .await
            .map_err(PullRequestStepError::github)?;
        if deletion == BranchDeletion::Deleted {
            tracing::info!(%branch, "deleted the merged run's branch");
        }
        Ok(())
    }

    /// The repository the Run works on, and its record.
    fn repository<'a>(
        input: &StepInput<'a>,
    ) -> Result<(CodeRepository, &'a Repository), PullRequestStepError> {
        let recorded = input.repository.ok_or(PullRequestStepError::NoRepository)?;
        let repository = CodeRepository::parse(&recorded.url)
            .ok_or_else(|| PullRequestStepError::InvalidRepository(recorded.url.clone()))?;
        Ok((repository, recorded))
    }

    /// The verify file the Run's base branch holds now, before the Run
    /// merged: the agent's branch changes it only as a Control File. The
    /// default when the base has none.
    pub(crate) async fn base_verify_file(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Result<VerifyFile, VerifyFileError>, PullRequestStepError> {
        let (repository, recorded) = Self::repository(input)?;
        let text = self
            .github
            .file_text(input.owner, &repository, &recorded.base_branch, VERIFY_FILE)
            .await
            .map_err(PullRequestStepError::github)?;
        Ok(VerifyFile::read(text.as_deref()))
    }

    /// The pull request from the Run's branch, if there is one. The Run
    /// keeps its page, which the board links to.
    async fn find(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
    ) -> Result<Option<PullRequest>, PullRequestStepError> {
        let found = self
            .github
            .find(input.owner, repository, &run_branch(input.key.run_id))
            .await
            .map_err(PullRequestStepError::github)?;
        if let Some(pull_request) = &found {
            self.keep_page(input, pull_request, &mut None).await?;
        }
        Ok(found)
    }

    /// Keep `pull_request`'s page on the Run, when GitHub names one. The
    /// page is how Macro's merge guard finds the Run that opened a pull
    /// request: without it the pull request reads as no Run's and merges on
    /// GitHub past the Run's merge Gate. So a write that fails fails the
    /// Step with a records error, which is retried; the write is idempotent.
    /// `kept` is the page this Step already kept, if any: a Step that reads
    /// the pull request again and again, as `ci` does, writes only when the
    /// page changes, so a records blip mid-wait does not start it over.
    async fn keep_page(
        &self,
        input: &StepInput<'_>,
        pull_request: &PullRequest,
        kept: &mut Option<String>,
    ) -> Result<(), PullRequestStepError> {
        if !pull_request.url.starts_with("https://")
            || kept.as_deref() == Some(pull_request.url.as_str())
        {
            return Ok(());
        }
        self.repo
            .set_pull_request_url(input.key.run_id, &pull_request.url)
            .await
            .map_err(PullRequestStepError::records)?;
        *kept = Some(pull_request.url.clone());
        Ok(())
    }

    /// Open the Run's pull request, or find the one it opened: `published`,
    /// `closed` when a person closed it without merging, or `refused` with
    /// GitHub's reason, such as a branch with nothing its base lacks. A
    /// published head's Control Files are checked, and named.
    pub async fn publish(&self, input: StepInput<'_>) -> Result<StepReport, PullRequestStepError> {
        let (repository, recorded) = Self::repository(&input)?;

        // A retry, or a Run sent back here after a fix, finds the pull
        // request it opened; the fix's push already updated it.
        let pull_request = match self.find(&input, &repository).await? {
            Some(pull_request) => pull_request,
            None => {
                let opening = self
                    .github
                    .open(
                        input.owner,
                        &repository,
                        &OpenPullRequest {
                            branch: run_branch(input.key.run_id),
                            base: recorded.base_branch.clone(),
                            title: title(input.request),
                            body: body(&input),
                        },
                    )
                    .await
                    .map_err(PullRequestStepError::github)?;

                // Opened, or refused because another attempt opened it
                // first: either way it is listed now, unless GitHub refused
                // for another reason.
                match (self.find(&input, &repository).await?, opening) {
                    (Some(pull_request), _) => pull_request,
                    (None, Opening::Refused(why)) => return Ok(report("refused", why)),
                    (None, Opening::Opened) => return Err(PullRequestStepError::NotListed),
                }
            }
        };

        Ok(match pull_request.state {
            PullRequestState::Closed => report(
                "closed",
                format!("{}\n\nIt was closed without merging.", named(&pull_request)),
            ),
            PullRequestState::Open => {
                let changes = self
                    .check_control_files(&input, &repository, recorded, &pull_request.head_sha)
                    .await?;
                let note = changes.as_ref().map(changes_note).unwrap_or_default();
                report("published", format!("{}{note}", named(&pull_request)))
            }
            PullRequestState::Merged => {
                let note = self.kept_note(&input).await?;
                report("published", format!("{}{note}", named(&pull_request)))
            }
        })
    }

    /// The pull request from the Run's branch with the checks on its head,
    /// read with one token; all its runs when `required` names any. Its
    /// page is kept unless `kept` already holds it.
    async fn find_with_checks(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        required: Option<&RequiredChecks>,
        kept: &mut Option<String>,
    ) -> Result<(PullRequest, HeadChecks), PullRequestStepError> {
        let all_runs = required.is_some_and(|required| !required.checks.is_empty());
        let (pull_request, checks) = self
            .github
            .find_with_checks(
                input.owner,
                repository,
                &run_branch(input.key.run_id),
                all_runs,
            )
            .await
            .map_err(PullRequestStepError::github)?
            .ok_or(PullRequestStepError::NoPullRequest)?;
        self.keep_page(input, &pull_request, kept).await?;
        Ok((pull_request, checks))
    }

    /// The checks the base branch requires, as far as they can be read.
    async fn required_checks(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        recorded: &Repository,
    ) -> Result<Option<RequiredChecks>, PullRequestStepError> {
        self.github
            .required_checks(input.owner, repository, &recorded.base_branch)
            .await
            .map_err(PullRequestStepError::github)
    }

    /// Wait for the checks on the head of the Run's pull request: `green`
    /// once the checks that count passed, as [`verdict`] says, on two reads
    /// in a row that saw the same checks, so a check that appears late is
    /// waited for; `red`
    /// with the failing ones as soon as one fails; `timeout` after
    /// [`CI_WAIT`]; or `closed` when a person closed the pull request. A pull
    /// request that merged meanwhile is `green`. The head it calls green is
    /// kept as the one a person may be asked to merge.
    pub async fn ci(&self, input: StepInput<'_>) -> Result<StepReport, PullRequestStepError> {
        let (repository, recorded) = Self::repository(&input)?;
        let required = self.required_checks(&input, &repository, recorded).await?;
        // The wall clock a suite's age is read against, moved on by the
        // waits, so a test's paused clock moves it too.
        let started = tokio::time::Instant::now();
        let started_at = Utc::now();
        let deadline = started + CI_WAIT;
        // The head and the checks the last read saw all pass, if it did.
        let mut last_green: Option<(String, Vec<String>)> = None;
        // Whether a read could not see the head's Actions runs.
        let mut actions_unread = false;
        // The page kept by an earlier read of this wait.
        let mut kept = None;
        loop {
            let (pull_request, checks) = self
                .find_with_checks(&input, &repository, required.as_ref(), &mut kept)
                .await?;
            let head = pull_request.head_sha.clone();
            actions_unread |= checks.actions_unread;
            let note = if actions_unread {
                ACTIONS_UNREAD_NOTE
            } else {
                ""
            };
            let elapsed = chrono::TimeDelta::from_std(started.elapsed()).unwrap_or_default();
            match verdict(
                &pull_request,
                &checks,
                required.as_ref(),
                started_at + elapsed,
            ) {
                Verdict::Green(passed) => {
                    let seen = (head.clone(), passed);
                    let merged = pull_request.state == PullRequestState::Merged;
                    if merged || last_green.as_ref() == Some(&seen) {
                        self.repo
                            .set_merge_head(input.key.run_id, &head)
                            .await
                            .map_err(PullRequestStepError::records)?;
                        return Ok(report(
                            "green",
                            format!(
                                "{}\n\nEvery check passed on {head}.{note}",
                                named(&pull_request)
                            ),
                        ));
                    }
                    last_green = Some(seen);
                }
                Verdict::Red(failed) => {
                    return Ok(report(
                        "red",
                        format!(
                            "{}\n\nThese checks failed on {head}:\n\n{}",
                            named(&pull_request),
                            failed.join("\n")
                        ),
                    ));
                }
                Verdict::Closed => {
                    return Ok(report(
                        "closed",
                        format!("{}\n\nIt was closed without merging.", named(&pull_request)),
                    ));
                }
                Verdict::Pending => last_green = None,
            }

            if tokio::time::Instant::now() + CI_POLL > deadline {
                return Ok(report(
                    "timeout",
                    format!(
                        "{}\n\nIts checks on {head} did not finish within {} minutes.{note}",
                        named(&pull_request),
                        CI_WAIT.as_secs() / 60
                    ),
                ));
            }
            tokio::time::sleep(CI_POLL).await;
        }
    }

    /// Before a person is asked to merge, check the pull request's head is
    /// still the commit `ci` called green, and its checks still all pass:
    /// `done`, which asks the person, who then merges exactly that commit;
    /// `changed` when the head moved since, as after a push, or a check no
    /// longer passes; `merged` or `closed` when a person already merged or
    /// closed it. Before a person is asked, the head's Control Files are
    /// checked, and named when the Run is Tampered: its Gate then needs
    /// them approved.
    pub async fn ready_to_merge(
        &self,
        input: StepInput<'_>,
    ) -> Result<StepReport, PullRequestStepError> {
        let (repository, recorded) = Self::repository(&input)?;
        let approved = self
            .repo
            .merge_head(input.key.run_id)
            .await
            .map_err(PullRequestStepError::records)?;
        let required = self.required_checks(&input, &repository, recorded).await?;
        let (pull_request, checks) = self
            .find_with_checks(&input, &repository, required.as_ref(), &mut None)
            .await?;

        match pull_request.state {
            PullRequestState::Merged => {
                return self.merged_outside(&input, recorded, &pull_request).await;
            }
            PullRequestState::Closed => {
                return Ok(report(
                    "closed",
                    format!("{}\n\nIt was closed without merging.", named(&pull_request)),
                ));
            }
            PullRequestState::Open => {}
        }
        let head = pull_request.head_sha.clone();
        let green = matches!(
            verdict(&pull_request, &checks, required.as_ref(), Utc::now()),
            Verdict::Green(_)
        );
        if approved.as_deref() != Some(head.as_str()) || !green {
            return Ok(report(
                "changed",
                format!(
                    "{}\n\nIts head is {head}, not the commit whose checks passed, or a check no longer passes.",
                    named(&pull_request)
                ),
            ));
        }

        // A person is never asked to merge a head that lacks a commit of
        // its base: the branch is brought up to date first, and its checks
        // and CI run again.
        if input.routes(BEHIND)
            && let Some(base_head) = self
                .behind_base(&input, &repository, recorded, &head)
                .await?
        {
            return Ok(behind_report(&pull_request, recorded, &base_head));
        }

        let changes = self
            .check_control_files(&input, &repository, recorded, &head)
            .await?;
        let note = changes.as_ref().map(changes_note).unwrap_or_default();
        Ok(report(
            "done",
            format!(
                "{}\n\nEvery check passed on {head}. Merging merges exactly that commit.{note}",
                named(&pull_request)
            ),
        ))
    }

    /// The head of the Run's base branch when `head` lacks it, so the
    /// branch is behind its base; none when `head` holds it. A base GitHub
    /// does not have fails the Step for good.
    async fn behind_base(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        recorded: &Repository,
        head: &str,
    ) -> Result<Option<String>, PullRequestStepError> {
        let base_head = self
            .github
            .branch_head(input.owner, repository, &recorded.base_branch)
            .await
            .map_err(PullRequestStepError::github)?
            .ok_or(PullRequestStepError::NoBase)?;
        let holds = self.contains(input, repository, &base_head, head).await?;
        Ok((!holds).then_some(base_head))
    }

    /// Whether the commit `head` of the Run's repository holds the commit
    /// `commit`.
    pub(crate) async fn holds(
        &self,
        input: &StepInput<'_>,
        commit: &str,
        head: &str,
    ) -> Result<bool, PullRequestStepError> {
        let (repository, _) = Self::repository(input)?;
        self.contains(input, &repository, commit, head).await
    }

    /// Whether the commit `head` holds the commit `commit`.
    async fn contains(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        commit: &str,
        head: &str,
    ) -> Result<bool, PullRequestStepError> {
        let merge_base = self
            .github
            .merge_base(input.owner, repository, commit, head)
            .await
            .map_err(PullRequestStepError::github)?;
        Ok(merge_base.as_deref() == Some(commit))
    }

    /// Wait until no Run is ahead of this one in its base's merge queue, up
    /// to `deadline`: whether its turn came. Which Runs are ahead is never
    /// told: they may be other people's, which this Run's readers may not
    /// see.
    async fn wait_for_turn(
        &self,
        input: &StepInput<'_>,
        deadline: tokio::time::Instant,
    ) -> Result<bool, PullRequestStepError> {
        loop {
            let ahead = self
                .repo
                .merge_queue_ahead(input.key.run_id, QUEUE_PLACE_KEPT)
                .await
                .map_err(PullRequestStepError::records)?;
            if ahead.is_empty() {
                return Ok(true);
            }
            if tokio::time::Instant::now() + QUEUE_POLL > deadline {
                return Ok(false);
            }
            tokio::time::sleep(QUEUE_POLL).await;
        }
    }

    /// Merge the Run's base into its branch, for the `update` Step: through
    /// GitHub's "Update branch", which adds a merge commit on top of the
    /// branch's head, only while the head is the one read, so it never
    /// rewrites the branch nor merges over a push made since. The update is
    /// kept before the Step goes on, so a retry goes on the way it started.
    ///
    /// GitHub merges after it answers, and may never do it (its merge failed
    /// later, or someone pushed in between): once its merge has not shown
    /// within [`UPDATE_SHOWS_WITHIN`], the branch is read again and GitHub
    /// asked again from the head it is at now, which replaces the update
    /// kept. One that still has not shown fails the attempt for a retry,
    /// which waits and asks the same way, so each attempt asks GitHub at
    /// most twice, and the Step's retries bound the whole.
    ///
    /// [`Update::Merged`] with the head GitHub's merge made (or the head, when
    /// it already held its base); [`Update::Conflict`] when GitHub could not
    /// merge them, for the agent to resolve; [`Update::Ended`] when the pull
    /// request was merged or closed meanwhile.
    pub(crate) async fn update_branch(
        &self,
        input: &StepInput<'_>,
    ) -> Result<Update, PullRequestStepError> {
        let (repository, recorded) = Self::repository(input)?;
        let kept = self
            .repo
            .branch_update(input.key)
            .await
            .map_err(PullRequestStepError::records)?;
        let update = match kept {
            Some(kept) => kept,
            None => {
                let asked = match self.ask_update(input, &repository, recorded).await? {
                    Asked::Ended(report) => return Ok(Update::Ended(report)),
                    Asked::Update(update) => update,
                };
                self.repo
                    .keep_branch_update(input.key, &asked)
                    .await
                    .map_err(PullRequestStepError::records)?
            }
        };
        if let Some(settled) = settled(&update) {
            return Ok(settled);
        }
        if let Some(shown) = self.merge_shown(input, &repository, update).await? {
            return Ok(shown);
        }

        // GitHub's merge never showed: read the branch, and ask again from
        // where it is now.
        let asked = match self.ask_update(input, &repository, recorded).await? {
            Asked::Ended(report) => return Ok(Update::Ended(report)),
            Asked::Update(update) => update,
        };
        let update = self
            .repo
            .restart_branch_update(input.key, &asked)
            .await
            .map_err(PullRequestStepError::records)?;
        if let Some(settled) = settled(&update) {
            return Ok(settled);
        }
        if let Some(shown) = self.merge_shown(input, &repository, update).await? {
            return Ok(shown);
        }
        Err(PullRequestStepError::BranchMoving(
            "github's merge of the base has not shown on it, though asked again".to_string(),
        ))
    }

    /// Read the Run's pull request, its branch's head and its base's, and
    /// ask GitHub to merge the base into the branch: the update to keep, or
    /// the Step's end when the pull request was merged or closed.
    async fn ask_update(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        recorded: &Repository,
    ) -> Result<Asked, PullRequestStepError> {
        let pull_request = self
            .find(input, repository)
            .await?
            .ok_or(PullRequestStepError::NoPullRequest)?;
        match pull_request.state {
            PullRequestState::Merged => {
                let merged = self.merged_outside(input, recorded, &pull_request).await?;
                return Ok(Asked::Ended(merged));
            }
            PullRequestState::Closed => {
                return Ok(Asked::Ended(report(
                    "closed",
                    format!("{}\n\nIt was closed without merging.", named(&pull_request)),
                )));
            }
            PullRequestState::Open => {}
        }
        let head = pull_request.head_sha.clone();
        let base_sha = self
            .github
            .branch_head(input.owner, repository, &recorded.base_branch)
            .await
            .map_err(PullRequestStepError::github)?
            .ok_or(PullRequestStepError::NoBase)?;
        if self.contains(input, repository, &base_sha, &head).await? {
            // Someone brought it up to date already: there is nothing to
            // merge, and the head is not a merge the factory made.
            return Ok(Asked::Update(BranchUpdate {
                base_sha,
                from_head: head.clone(),
                conflict: false,
                updated_head: Some(head),
            }));
        }
        let updating = self
            .github
            .update_branch(input.owner, repository, pull_request.number, &head)
            .await
            .map_err(PullRequestStepError::github)?;
        let conflict = match updating {
            BranchUpdating::Started => false,
            BranchUpdating::Conflict(why) => {
                tracing::info!(%why, "the run's base conflicts with its branch");
                true
            }
            BranchUpdating::Declined(why) => {
                return Err(PullRequestStepError::UpdateDeclined(why));
            }
            BranchUpdating::HeadMoved => {
                return Err(PullRequestStepError::BranchMoving(
                    "someone pushed to it".to_string(),
                ));
            }
        };
        Ok(Asked::Update(BranchUpdate {
            base_sha,
            from_head: head,
            conflict,
            updated_head: None,
        }))
    }

    /// Wait up to [`UPDATE_SHOWS_WITHIN`] for GitHub's merge of `update`
    /// to show: the branch moved from its start to a commit that holds the
    /// base. The update with that head, kept; none when it did not show.
    async fn merge_shown(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        update: BranchUpdate,
    ) -> Result<Option<Update>, PullRequestStepError> {
        let deadline = tokio::time::Instant::now() + UPDATE_SHOWS_WITHIN;
        loop {
            let head = self
                .github
                .branch_head(input.owner, repository, &run_branch(input.key.run_id))
                .await
                .map_err(PullRequestStepError::github)?;
            if let Some(head) = head
                && head != update.from_head
                && self
                    .contains(input, repository, &update.base_sha, &head)
                    .await?
            {
                self.repo
                    .set_updated_head(input.key, &head)
                    .await
                    .map_err(PullRequestStepError::records)?;
                return Ok(Some(Update::Merged(BranchUpdate {
                    updated_head: Some(head),
                    ..update
                })));
            }
            if tokio::time::Instant::now() + UPDATE_POLL > deadline {
                return Ok(None);
            }
            tokio::time::sleep(UPDATE_POLL).await;
        }
    }

    /// Wait while the Main Hold on the Run's base `recorded` lasts, up to
    /// [`HOLD_WAIT`]: none once it clears, or the hold still set then. A
    /// held base has its watch started, as the Run's owner, in case none
    /// runs: the watch is what clears it.
    async fn wait_out_hold(
        &self,
        input: &StepInput<'_>,
        recorded: &Repository,
        deadline: tokio::time::Instant,
    ) -> Result<Option<MainHold>, PullRequestStepError> {
        let mut watched = false;
        loop {
            let Some(hold) = self
                .repo
                .main_hold(recorded)
                .await
                .map_err(PullRequestStepError::records)?
            else {
                return Ok(None);
            };
            if !watched {
                self.watches
                    .watch(&MainWatch::of(input.key.run_id, recorded))
                    .await
                    .map_err(PullRequestStepError::watch)?;
                watched = true;
            }
            if tokio::time::Instant::now() + HOLD_POLL > deadline {
                return Ok(Some(hold));
            }
            tokio::time::sleep(HOLD_POLL).await;
        }
    }

    /// Record that the Run merged `pull_request` into its base `recorded`,
    /// and watch the base's checks, which the merge may break. The pull
    /// request is merged either way: a watch that does not start is logged,
    /// never the Step's failure, and the next merge or held merge to the
    /// base starts it.
    async fn merged(
        &self,
        input: &StepInput<'_>,
        recorded: &Repository,
        pull_request: &PullRequest,
    ) -> Result<(), PullRequestStepError> {
        self.repo
            .record_merge(
                input.key.run_id,
                pull_request.number,
                &pull_request.url,
                Utc::now(),
            )
            .await
            .map_err(PullRequestStepError::records)?;
        let watched = self
            .watches
            .watch(&MainWatch::of(input.key.run_id, recorded))
            .await;
        if let Err(error) = watched {
            tracing::warn!(?error, "the merged base's watch did not start");
        }
        Ok(())
    }

    /// Merge the commit a person agreed to merge: `merged`, also when it
    /// already was; `changed` when the pull request's head moved since, so
    /// its checks run again; `refused` with GitHub's reason; or `closed`.
    /// A Tampered commit merges only when the latest answer to a Gate of the
    /// Run, the one that agreed to the merge, approved every Control File
    /// it changes without its request asking: otherwise it is `changed`, so
    /// the Run goes back through `ci` to the merge Gate, which shows the
    /// files. That route is in every `code` Flow a Run carries, so Runs in
    /// flight take it too, as a Run that agreed before its files were read.
    ///
    /// While the base's required checks fail, a Main Hold, it waits for
    /// them to pass, and ends `held` if they still fail after
    /// [`HOLD_WAIT`], which the `code` Flow sends back to its merge Gate.
    /// A merge is recorded, and the base's checks watched.
    ///
    /// On a Flow that routes them, it merges in its base's merge queue: it
    /// waits for the Runs ahead of it, up to [`QUEUE_WAIT`], then ends
    /// `queued`, which the `code` Flow sends back to `merge` with the
    /// approval it has; it merges only while the head approved still has
    /// its checks passing (else `changed`), and it ends `behind` rather than
    /// merge a head that lacks a commit of its base. A Run that leaves its merge without merging keeps
    /// its place for a while; one that merged leaves the queue.
    pub async fn merge(&self, input: StepInput<'_>) -> Result<StepReport, PullRequestStepError> {
        let run_id = input.key.run_id;
        let queued = input.routes(QUEUED);
        let merged = self.merge_in_queue(input).await?;
        if queued {
            if merged.outcome.as_str() == "merged" {
                self.repo
                    .drop_from_merge_queue(run_id)
                    .await
                    .map_err(PullRequestStepError::records)?;
            } else {
                self.repo
                    .leave_merge_queue(run_id)
                    .await
                    .map_err(PullRequestStepError::records)?;
            }
        }
        Ok(merged)
    }

    /// [`Self::merge`], before the Run's place in its base's queue is
    /// given up or kept.
    async fn merge_in_queue(
        &self,
        input: StepInput<'_>,
    ) -> Result<StepReport, PullRequestStepError> {
        let (repository, recorded) = Self::repository(&input)?;
        let head = self
            .repo
            .merge_head(input.key.run_id)
            .await
            .map_err(PullRequestStepError::records)?
            .ok_or(PullRequestStepError::NoMergeHead)?;
        let pull_request = self
            .find(&input, &repository)
            .await?
            .ok_or(PullRequestStepError::NoPullRequest)?;

        // A retry whose merge GitHub took, though its answer was lost, finds
        // the pull request merged.
        match pull_request.state {
            PullRequestState::Merged => {
                return self.merged_outside(&input, recorded, &pull_request).await;
            }
            PullRequestState::Closed => {
                return Ok(report(
                    "closed",
                    format!("{}\n\nIt was closed without merging.", named(&pull_request)),
                ));
            }
            PullRequestState::Open => {}
        }

        let changes = self
            .check_control_files(&input, &repository, recorded, &head)
            .await?;
        if let Some(changes) = changes.as_ref().filter(|changes| changes.is_tampered()) {
            let approved = input
                .earlier
                .iter()
                .rev()
                .find_map(|step| step.gate.as_ref()?.answer.as_ref())
                .and_then(|answer| answer.approved_control_files.as_deref());
            if !approved.is_some_and(|approved| changes.approved_by(approved)) {
                return Ok(report(
                    "changed",
                    format!(
                        "{}\n\nIt was not merged: no person approved the Control Files it changes. \
                         The Run goes back to its checks, then to the merge Gate, unless that \
                         Gate already asked its last time.{}",
                        named(&pull_request),
                        changes_note(changes)
                    ),
                ));
            }
        }

        // The hold's wait and the queue's share one deadline, short of the
        // Step attempt's 30 minutes.
        let started = tokio::time::Instant::now();
        if let Some(hold) = self
            .wait_out_hold(&input, recorded, started + HOLD_WAIT)
            .await?
        {
            return Ok(report(
                "held",
                format!(
                    "{}\n\n{}",
                    named(&pull_request),
                    held_note(&recorded.base_branch, &hold)
                ),
            ));
        }

        if input.routes(QUEUED) {
            self.repo
                .enter_merge_queue(input.key.run_id)
                .await
                .map_err(PullRequestStepError::records)?;
            // The Runs ahead may be anyone's: what this Run's readers see
            // names none of them, nor how many there are.
            if !self.wait_for_turn(&input, started + QUEUE_WAIT).await? {
                return Ok(report(
                    QUEUED,
                    format!(
                        "{}\n\nIt was not merged yet: runs that reached their merge into {} before \
                         it have not merged yet. It keeps its place in the queue and goes on waiting \
                         with the approval it has, while its head and base stay as approved.",
                        named(&pull_request),
                        recorded.base_branch,
                    ),
                ));
            }
            // A hold set while it waited holds it still: it waits no more,
            // as its time went to the queue, and goes back to its Gate.
            let held = self
                .repo
                .main_hold(recorded)
                .await
                .map_err(PullRequestStepError::records)?;
            if let Some(hold) = held {
                return Ok(report(
                    "held",
                    format!(
                        "{}\n\n{}",
                        named(&pull_request),
                        held_note(&recorded.base_branch, &hold)
                    ),
                ));
            }
        }

        // A merge that waited its turn merges the head approved only while
        // its checks still pass: a check that failed since (a re-run) goes
        // back through `ci` to the merge Gate.
        if input.routes(QUEUED) {
            let required = self.required_checks(&input, &repository, recorded).await?;
            let (now, checks) = self
                .find_with_checks(&input, &repository, required.as_ref(), &mut None)
                .await?;
            let green = matches!(
                verdict(&now, &checks, required.as_ref(), Utc::now()),
                Verdict::Green(_)
            );
            if now.state == PullRequestState::Open && (now.head_sha != head || !green) {
                return Ok(report(
                    "changed",
                    format!(
                        "{}\n\nIt was not merged: its head is {}, not {head}, the commit approved, \
                         or a check no longer passes; its checks run again.",
                        named(&now),
                        now.head_sha
                    ),
                ));
            }
        }

        // The head agreed to is merged only while it holds every commit of
        // its base: one the base gained since its checks passed was never
        // checked with the change.
        if input.routes(BEHIND)
            && let Some(base_head) = self
                .behind_base(&input, &repository, recorded, &head)
                .await?
        {
            return Ok(behind_report(&pull_request, recorded, &base_head));
        }

        let merging = self
            .github
            .merge(input.owner, &repository, pull_request.number, &head)
            .await
            .map_err(PullRequestStepError::github)?;
        // GitHub refuses a merge whose base moved under a branch protection
        // that wants branches up to date: that is a branch behind its base.
        if let Merging::Refused(_) = &merging
            && input.routes(BEHIND)
            && let Some(base_head) = self
                .behind_base(&input, &repository, recorded, &head)
                .await?
        {
            return Ok(behind_report(&pull_request, recorded, &base_head));
        }
        if merging == Merging::Merged {
            self.merged(&input, recorded, &pull_request).await?;
            self.delete_merged_branch(&input, &head, Merger::Run).await;
        }
        Ok(match merging {
            Merging::Merged => report(
                "merged",
                format!("{}\n\nMerged {head}.", named(&pull_request)),
            ),
            Merging::HeadChanged => report(
                "changed",
                format!(
                    "{}\n\nIts head moved from {head}, the commit agreed to; its checks run again.",
                    named(&pull_request)
                ),
            ),
            Merging::Refused(why) => report(
                "refused",
                format!("{}\n\nGitHub did not merge it: {why}", named(&pull_request)),
            ),
        })
    }
}
