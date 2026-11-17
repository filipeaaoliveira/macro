//! The Main Hold: while the required checks on a base branch the factory
//! merges to fail, no Run merges to it.
//!
//! After each merge, a watch of the base (one Temporal workflow per base,
//! `inbound::temporal::main_watch`) looks at the checks on its head, then
//! again every few minutes while they run or fail. A red look holds the base
//! and files one task per red period naming the factory merges made since
//! the base was last green; a green look clears the hold, re-runs that pass
//! included. `merge` waits while its base is held (`pull_request_steps`).

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use factory_records::domain::main_hold::{FactoryMerge, MainHold, MainHoldRepository};
use factory_records::domain::models::{Repository, Run, RunId};
use factory_records::domain::ports::RunRepository;
use github::domain::models::MacroTaskId;
use serde::{Deserialize, Serialize};

use crate::domain::models::{CheckState, CodeRepository, HeadChecks, RequiredChecks, StepError};
use crate::domain::ports::{HoldTasks, MainWatchService, PullRequests, Retryable};
use crate::domain::pull_request_steps::{Verdict, checks_verdict, required_passed};

/// The most suspect merges a hold's task names: the newest.
pub const SUSPECTS_LISTED: u16 = 50;

/// The most bytes of failing checks a hold keeps.
const MAX_FAILED_BYTES: usize = 60_000;

/// The base branch of a Run that merges, to watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainWatch {
    /// The repository's `https://github.com/owner/name` URL.
    pub repo_url: String,
    /// The branch Runs merge to.
    pub base_branch: String,
    /// The Run that started the watch: as its owner the watch reads GitHub,
    /// and in its task's initiative the hold's task is filed.
    pub run_id: RunId,
}

impl MainWatch {
    /// The watch of the base `repository` the Run `run_id` merges to.
    pub fn of(run_id: RunId, repository: &Repository) -> Self {
        Self {
            repo_url: repository.url.clone(),
            base_branch: repository.base_branch.clone(),
            run_id,
        }
    }

    /// The base branch it watches.
    pub fn base(&self) -> Repository {
        Repository {
            url: self.repo_url.clone(),
            base_branch: self.base_branch.clone(),
        }
    }

    /// The id of its workflow: one per base branch, so a second merge finds
    /// the watch the first one started.
    pub fn workflow_id(&self) -> String {
        format!("factory-main-watch:{}:{}", self.repo_url, self.base_branch)
    }
}

/// What the checks on a base branch's head said on one look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MainState {
    /// Every required check passed: the base holds nothing.
    Green,
    /// A required check failed: the base is held.
    Red,
    /// A required check has not finished, or has not appeared yet.
    Pending,
    /// The Run's owner reaches no such branch.
    Gone,
}

/// One look at a base branch, for its watch to decide whether to go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainLook {
    /// What its checks said.
    pub state: MainState,
    /// Whether the base is held after the look.
    pub held: bool,
}

/// The task a hold files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldTask {
    /// Its name.
    pub title: String,
    /// Its text, as markdown.
    pub markdown: String,
}

/// What the checks on a base branch's head say: as its `required` checks
/// say when they name any, else as a pull request's checks would.
///
/// A required check is red when it failed and no run or status of it
/// passed: a re-run that passes clears it. One that has not finished, or
/// not appeared, keeps the base pending.
pub(crate) fn base_verdict(
    checks: &HeadChecks,
    required: Option<&RequiredChecks>,
    now: DateTime<Utc>,
) -> Verdict {
    let named = required.map_or(&[][..], |required| &required.checks[..]);
    if named.is_empty() {
        return checks_verdict(checks, required, now);
    }

    let mut failed = Vec::new();
    let mut pending = false;
    for check in named {
        if required_passed(check, checks) {
            continue;
        }
        let runs = checks.runs.iter().filter(|run| {
            run.name == check.name && check.app_id.is_none_or(|app_id| run.app_id == Some(app_id))
        });
        let mut lines: Vec<String> = runs
            .filter_map(|run| match &run.state {
                CheckState::Failed(conclusion) => Some(format!(
                    "- {} ({conclusion}): {}",
                    run.name,
                    run.url.as_deref().unwrap_or("no page")
                )),
                _ => None,
            })
            .collect();
        if check.app_id.is_none() {
            lines.extend(checks.statuses.iter().filter_map(|status| {
                match (&status.state, status.context == check.name) {
                    (CheckState::Failed(state), true) => Some(format!(
                        "- {} ({state}): {}",
                        status.context,
                        status.url.as_deref().unwrap_or("no page")
                    )),
                    _ => None,
                }
            }));
        }
        if lines.is_empty() {
            pending = true;
        }
        failed.extend(lines);
    }

    if !failed.is_empty() {
        Verdict::Red(failed)
    } else if pending {
        Verdict::Pending
    } else {
        Verdict::Green(named.iter().map(|check| check.name.clone()).collect())
    }
}

/// The failing checks' lines, cut to what a hold keeps.
fn failed_text(failed: &[String]) -> String {
    let mut text = String::new();
    for line in failed {
        if text.len() + line.len() + 1 > MAX_FAILED_BYTES {
            text.push_str("- (more checks failed)");
            break;
        }
        text.push_str(line);
        text.push('\n');
    }
    text.trim_end().to_string()
}

/// The task a hold on `branch` of `repository` files, naming the merges
/// `suspects` made since it was last green, oldest first.
pub(crate) fn hold_task(
    repository: &CodeRepository,
    branch: &str,
    hold: &MainHold,
    suspects: &[FactoryMerge],
) -> HoldTask {
    let name = format!("{}/{}", repository.owner(), repository.name());
    let mut markdown = format!(
        "The required checks on `{branch}` of [{name}]({url}) fail on {head}, so the \
         factory merges nothing to it until they pass again. The hold clears by itself \
         once they do, re-runs included.\n\nFailing checks:\n\n{failed}\n\n",
        url = repository.url(),
        head = hold.head_sha,
        failed = hold.failed,
    );
    if suspects.is_empty() {
        markdown.push_str(&format!(
            "No factory run merged to `{branch}` since its checks last passed."
        ));
    } else {
        markdown.push_str(&format!(
            "Factory merges to `{branch}` since its checks last passed, oldest first:\n\n"
        ));
        for (place, merge) in suspects.iter().enumerate() {
            markdown.push_str(&format!(
                "{}. [Pull request #{}]({}), merged {} by run {}\n",
                place + 1,
                merge.number,
                merge.url,
                merge.merged_at.format("%Y-%m-%d %H:%M UTC"),
                merge.run_id,
            ));
        }
    }
    HoldTask {
        title: format!("{branch} is red in {name}"),
        markdown: markdown.trim_end().to_string(),
    }
}

/// What a Run's `merge` tells its owner when its base is held.
pub(crate) fn held_note(branch: &str, hold: &MainHold) -> String {
    let mut note = format!(
        "The required checks on `{branch}` fail on {} since {}, so the factory merges \
         nothing to it until they pass again:\n\n{}",
        hold.head_sha,
        hold.since.format("%Y-%m-%d %H:%M UTC"),
        hold.failed,
    );
    if let Some(task_id) = hold.task_id {
        note.push_str(&format!(
            "\n\nThe merges that may have caused it: {}",
            MacroTaskId::from_uuid(&task_id).to_task_id_string()
        ));
    }
    note
}

fn internal(error: impl std::error::Error + Send + Sync + 'static) -> StepError {
    StepError::Internal(rootcause::Report::new(error).into_dynamic())
}

fn github(error: impl std::error::Error + Retryable + Send + Sync + 'static) -> StepError {
    let retryable = error.retryable();
    let report = rootcause::Report::new(error).into_dynamic();
    if retryable {
        StepError::Internal(report)
    } else {
        StepError::Final(report)
    }
}

/// The watch's looks at a base branch, over the Run records, GitHub as the
/// watch's Run's owner, and the tasks a hold files.
#[derive(Debug, Clone)]
pub struct MainWatchServiceImpl<R, G, T> {
    repo: R,
    github: G,
    tasks: T,
}

impl<R, G, T> MainWatchServiceImpl<R, G, T>
where
    R: RunRepository + MainHoldRepository,
    G: PullRequests,
    T: HoldTasks,
{
    /// The looks, over `repo`, `github` and `tasks`.
    pub fn new(repo: R, github: G, tasks: T) -> Self {
        Self {
            repo,
            github,
            tasks,
        }
    }

    /// File the task of `hold` on `base`, once per red period: the claim
    /// comes first, so a second look or a retry files none, and a failed
    /// filing gives it back for the next look.
    async fn file_task(
        &self,
        run: &Run,
        base: &Repository,
        repository: &CodeRepository,
        hold: &MainHold,
    ) -> Result<(), StepError> {
        let claimed = self
            .repo
            .claim_hold_task(base, hold.since)
            .await
            .map_err(internal)?;
        if !claimed {
            return Ok(());
        }

        let suspects = self
            .repo
            .suspect_merges(base, SUSPECTS_LISTED)
            .await
            .map_err(internal)?;
        let task = hold_task(repository, &base.base_branch, hold, &suspects);
        match self.tasks.file(&run.user_id, run.task_id, &task).await {
            Ok(task_id) => self
                .repo
                .record_hold_task(base, hold.since, task_id)
                .await
                .map_err(internal),
            Err(error) => {
                self.repo
                    .release_hold_task(base, hold.since)
                    .await
                    .map_err(internal)?;
                Err(internal(error))
            }
        }
    }
}

impl<R, G, T> MainWatchService for MainWatchServiceImpl<R, G, T>
where
    R: RunRepository + MainHoldRepository,
    G: PullRequests,
    T: HoldTasks,
{
    #[tracing::instrument(err, skip_all, fields(repo_url = %watch.repo_url, base = %watch.base_branch))]
    async fn look(&self, watch: MainWatch) -> Result<MainLook, StepError> {
        let base = watch.base();
        let repository = CodeRepository::parse(&watch.repo_url).ok_or_else(|| {
            StepError::Final(rootcause::report!("{} names no repository", watch.repo_url))
        })?;
        let run = self
            .repo
            .get_run(watch.run_id)
            .await
            .map_err(internal)?
            .ok_or(StepError::NoSuchRun)?;

        let required = self
            .github
            .required_checks(&run.user_id, &repository, &base.base_branch)
            .await
            .map_err(github)?;
        let all_runs = required
            .as_ref()
            .is_some_and(|required| !required.checks.is_empty());
        // A merge recorded after this read started is not covered by what
        // it saw, so a green look counts from here.
        let seen_at = Utc::now();
        let read = self
            .github
            .branch_checks(&run.user_id, &repository, &base.base_branch, all_runs)
            .await
            .map_err(github)?;
        let Some((head, checks)) = read else {
            return Ok(MainLook {
                state: MainState::Gone,
                held: false,
            });
        };

        match base_verdict(&checks, required.as_ref(), seen_at) {
            Verdict::Green(_) => {
                self.repo
                    .clear_main_hold(&base, seen_at)
                    .await
                    .map_err(internal)?;
                Ok(MainLook {
                    state: MainState::Green,
                    held: false,
                })
            }
            Verdict::Red(failed) => {
                let hold = self
                    .repo
                    .hold_main(&base, &head, &failed_text(&failed))
                    .await
                    .map_err(internal)?;
                if !hold.task_claimed {
                    self.file_task(&run, &base, &repository, &hold).await?;
                }
                Ok(MainLook {
                    state: MainState::Red,
                    held: true,
                })
            }
            Verdict::Pending | Verdict::Closed => {
                let held = self
                    .repo
                    .main_hold(&base)
                    .await
                    .map_err(internal)?
                    .is_some();
                Ok(MainLook {
                    state: MainState::Pending,
                    held,
                })
            }
        }
    }
}
