//! The Steps that take a Run's branch to a merged pull request, on GitHub as
//! the Run's owner: `publish` opens the pull request, `ci` waits for its
//! checks, `ready_to_merge` shows a person the commit that passed them, and
//! `merge` merges that commit once they agree.
//!
//! Each reads GitHub before it changes anything, so a Temporal retry, or a
//! Run sent back to a Step it took before, finds the pull request it opened
//! or merged instead of opening or merging another.

#[cfg(test)]
mod test;

use std::time::Duration;

use chrono::{DateTime, Utc};
use factory_records::domain::models::{Outcome, Repository};
use factory_records::domain::ports::RunRepository;
use github::domain::models::MacroTaskId;

use crate::domain::models::{
    CheckState, CheckSuite, CodeRepository, HeadChecks, Merging, OpenPullRequest, Opening,
    PullRequest, PullRequestState, RequiredCheck, RequiredChecks, StepInput, StepReport,
    run_branch,
};
use crate::domain::ports::{PullRequests, Retryable};

/// How long `ci` waits for a pull request's checks to finish: short of a
/// Step attempt's 30 minutes, so the Step ends `timeout` rather than failing.
pub const CI_WAIT: Duration = Duration::from_secs(25 * 60);

/// How long `ci` waits between two reads of the checks.
const CI_POLL: Duration = Duration::from_secs(20);

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
const EMPTY_SUITE_GRACE: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

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
}

impl PullRequestStepError {
    fn github(error: impl std::error::Error + Retryable + Send + Sync + 'static) -> Self {
        Self::Github {
            retryable: error.retryable(),
            report: rootcause::Report::new(error).into_dynamic(),
        }
    }

    fn records(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Records(rootcause::Report::new(error).into_dynamic())
    }
}

impl Retryable for PullRequestStepError {
    /// GitHub may list a pull request it just opened a moment later, and a
    /// failed read or write of the records is the database's.
    fn retryable(&self) -> bool {
        match self {
            Self::NotListed | Self::Records(_) => true,
            Self::Github { retryable, .. } => *retryable,
            Self::NoRepository
            | Self::InvalidRepository(_)
            | Self::NoPullRequest
            | Self::NoMergeHead => false,
        }
    }
}

/// What a pull request's checks on its head say.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
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
fn required_passed(check: &RequiredCheck, checks: &HeadChecks) -> bool {
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
        PullRequestState::Merged => return Verdict::Green(Vec::new()),
        PullRequestState::Closed => return Verdict::Closed,
        PullRequestState::Open => {}
    }

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
/// person is asked to merge in the Run's records.
#[derive(Debug, Clone)]
pub struct PullRequestSteps<G, R> {
    github: G,
    repo: R,
}

impl<G: PullRequests, R: RunRepository> PullRequestSteps<G, R> {
    /// The Steps, over `github` and the Run records `repo`.
    pub fn new(github: G, repo: R) -> Self {
        Self { github, repo }
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

    /// The pull request from the Run's branch, if there is one.
    async fn find(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
    ) -> Result<Option<PullRequest>, PullRequestStepError> {
        self.github
            .find(input.owner, repository, &run_branch(input.key.run_id))
            .await
            .map_err(PullRequestStepError::github)
    }

    /// Open the Run's pull request, or find the one it opened: `published`,
    /// `closed` when a person closed it without merging, or `refused` with
    /// GitHub's reason, such as a branch with nothing its base lacks.
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
            PullRequestState::Open | PullRequestState::Merged => {
                report("published", named(&pull_request))
            }
        })
    }

    /// The pull request from the Run's branch with the checks on its head,
    /// read with one token; all its runs when `required` names any.
    async fn find_with_checks(
        &self,
        input: &StepInput<'_>,
        repository: &CodeRepository,
        required: Option<&RequiredChecks>,
    ) -> Result<(PullRequest, HeadChecks), PullRequestStepError> {
        let all_runs = required.is_some_and(|required| !required.checks.is_empty());
        self.github
            .find_with_checks(
                input.owner,
                repository,
                &run_branch(input.key.run_id),
                all_runs,
            )
            .await
            .map_err(PullRequestStepError::github)?
            .ok_or(PullRequestStepError::NoPullRequest)
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
        loop {
            let (pull_request, checks) = self
                .find_with_checks(&input, &repository, required.as_ref())
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
    /// closed it.
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
            .find_with_checks(&input, &repository, required.as_ref())
            .await?;

        match pull_request.state {
            PullRequestState::Merged => {
                return Ok(report("merged", named(&pull_request)));
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

        Ok(report(
            "done",
            format!(
                "{}\n\nEvery check passed on {head}. Merging merges exactly that commit.",
                named(&pull_request)
            ),
        ))
    }

    /// Merge the commit a person agreed to merge: `merged`, also when it
    /// already was; `changed` when the pull request's head moved since, so
    /// its checks run again; `refused` with GitHub's reason; or `closed`.
    pub async fn merge(&self, input: StepInput<'_>) -> Result<StepReport, PullRequestStepError> {
        let (repository, _) = Self::repository(&input)?;
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
            PullRequestState::Merged => return Ok(report("merged", named(&pull_request))),
            PullRequestState::Closed => {
                return Ok(report(
                    "closed",
                    format!("{}\n\nIt was closed without merging.", named(&pull_request)),
                ));
            }
            PullRequestState::Open => {}
        }

        let merging = self
            .github
            .merge(input.owner, &repository, pull_request.number, &head)
            .await
            .map_err(PullRequestStepError::github)?;
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
