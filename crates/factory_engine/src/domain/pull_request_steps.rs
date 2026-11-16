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

use factory_records::domain::models::{Outcome, Repository};
use factory_records::domain::ports::RunRepository;
use github::domain::models::MacroTaskId;

use crate::domain::models::{
    Check, CodeRepository, Merging, OpenPullRequest, Opening, PullRequest, PullRequestState,
    StepInput, StepReport, run_branch,
};
use crate::domain::ports::{PullRequests, Retryable};

/// How long `ci` waits for a pull request's checks to finish: short of a
/// Step attempt's 30 minutes, so the Step ends `timeout` rather than failing.
pub const CI_WAIT: Duration = Duration::from_secs(25 * 60);

/// How long `ci` waits between two reads of the checks.
const CI_POLL: Duration = Duration::from_secs(20);

/// The longest pull request title the Steps write.
const MAX_TITLE_CHARS: usize = 72;

/// The conclusions of a completed check that let a pull request merge.
const PASSING: [&str; 3] = ["success", "neutral", "skipped"];

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
    /// Some check has not finished, or none was reported yet.
    Pending,
    /// Every check passed, or the pull request merged.
    Green,
    /// These checks failed.
    Red(Vec<Check>),
    /// The pull request was closed without merging.
    Closed,
}

/// What the checks of `pull_request` say. Checks that could not be read, or
/// a head with none yet, wait: CI may not have reported on a fresh push.
fn verdict(pull_request: &PullRequest) -> Verdict {
    match pull_request.state {
        PullRequestState::Merged => return Verdict::Green,
        PullRequestState::Closed => return Verdict::Closed,
        PullRequestState::Open => {}
    }
    let Some(checks) = pull_request
        .checks
        .as_ref()
        .filter(|checks| !checks.is_empty())
    else {
        return Verdict::Pending;
    };

    let failed: Vec<Check> = checks
        .iter()
        .filter(|check| {
            check.status == "completed"
                && !check
                    .conclusion
                    .as_deref()
                    .is_some_and(|conclusion| PASSING.contains(&conclusion))
        })
        .cloned()
        .collect();
    if !failed.is_empty() {
        return Verdict::Red(failed);
    }
    if checks.iter().all(|check| check.status == "completed") {
        Verdict::Green
    } else {
        Verdict::Pending
    }
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
fn body(input: &StepInput<'_>) -> String {
    let mut body = format!(
        "{}\n\nOpened by a Macro factory run ({}).",
        input.request, input.key.run_id
    );
    if let Some(task_id) = input.task_id {
        body.push_str(&format!(
            "\n\nTask: {}",
            MacroTaskId::from_uuid(&task_id).to_task_id_string()
        ));
    }
    body
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

    /// Wait for the checks on the head of the Run's pull request: `green`
    /// once they all passed, `red` with the failing ones as soon as one
    /// fails, `timeout` after [`CI_WAIT`], or `closed` when a person closed
    /// the pull request. A pull request that merged meanwhile is `green`.
    pub async fn ci(&self, input: StepInput<'_>) -> Result<StepReport, PullRequestStepError> {
        let (repository, _) = Self::repository(&input)?;
        let deadline = tokio::time::Instant::now() + CI_WAIT;
        loop {
            let pull_request = self
                .find(&input, &repository)
                .await?
                .ok_or(PullRequestStepError::NoPullRequest)?;
            let head = pull_request.head_sha.as_deref().unwrap_or("its head");
            match verdict(&pull_request) {
                Verdict::Green => {
                    return Ok(report(
                        "green",
                        format!("{}\n\nEvery check passed on {head}.", named(&pull_request)),
                    ));
                }
                Verdict::Red(failed) => {
                    let mut output = format!(
                        "{}\n\nThese checks failed on {head}:\n",
                        named(&pull_request)
                    );
                    for check in failed {
                        let conclusion = check.conclusion.as_deref().unwrap_or("no conclusion");
                        let url = check.url.as_deref().unwrap_or("no page");
                        output.push_str(&format!("\n- {} ({conclusion}): {url}", check.name));
                    }
                    return Ok(report("red", output));
                }
                Verdict::Closed => {
                    return Ok(report(
                        "closed",
                        format!("{}\n\nIt was closed without merging.", named(&pull_request)),
                    ));
                }
                Verdict::Pending => {}
            }

            if tokio::time::Instant::now() + CI_POLL > deadline {
                return Ok(report(
                    "timeout",
                    format!(
                        "{}\n\nIts checks on {head} did not finish within {} minutes.",
                        named(&pull_request),
                        CI_WAIT.as_secs() / 60
                    ),
                ));
            }
            tokio::time::sleep(CI_POLL).await;
        }
    }

    /// Before a person is asked to merge, check the pull request's head is
    /// still the commit whose checks passed, and keep it as the one commit
    /// that may merge: `done`, which asks the person; `changed` when the
    /// head's checks no longer all passed, as after a push; `merged` or
    /// `closed` when a person already merged or closed it.
    pub async fn ready_to_merge(
        &self,
        input: StepInput<'_>,
    ) -> Result<StepReport, PullRequestStepError> {
        let (repository, _) = Self::repository(&input)?;
        let pull_request = self
            .find(&input, &repository)
            .await?
            .ok_or(PullRequestStepError::NoPullRequest)?;

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
        let head = match (&pull_request.head_sha, verdict(&pull_request)) {
            (Some(head), Verdict::Green) => head.clone(),
            _ => {
                return Ok(report(
                    "changed",
                    format!(
                        "{}\n\nIts head changed since its checks passed.",
                        named(&pull_request)
                    ),
                ));
            }
        };

        self.repo
            .set_merge_head(input.key.run_id, &head)
            .await
            .map_err(PullRequestStepError::records)?;
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
