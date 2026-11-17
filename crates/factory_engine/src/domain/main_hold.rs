//! The Main Hold: while the required checks on a base branch the factory
//! merges to fail, no Run merges to it.
//!
//! Each merge signals the watch of its base (one Temporal workflow per base,
//! `inbound::temporal::main_watch`), starting it when none runs. The watch
//! looks at the checks the base requires on its head, then again every few
//! minutes. A red look holds the base and files one task per red period
//! naming the factory merges made since the base was last green; a green
//! look clears the hold, re-runs that pass included. A base whose required
//! checks cannot be read, or name none, is not held. `merge` waits while its
//! base is held (`pull_request_steps`).

#[cfg(test)]
mod test;

use chrono::{TimeDelta, Utc};
use factory_records::domain::main_hold::{FactoryMerge, MainHold, MainHoldRepository};
use factory_records::domain::models::{Repository, Run, RunId};
use factory_records::domain::ports::RunRepository;
use github::domain::models::MacroTaskId;
use serde::{Deserialize, Serialize};

use crate::domain::models::{CheckState, CodeRepository, HeadChecks, RequiredCheck, StepError};
use crate::domain::ports::{HoldTasks, MainWatchService, PullRequests};
use crate::domain::pull_request_steps::{Verdict, required_passed};

/// The most suspect merges a hold's task names: the newest.
pub const SUSPECTS_LISTED: u16 = 50;

/// How long after the base's latest merge a watch that holds nothing stops:
/// long enough for slow checks on the merge to finish.
pub const IDLE_AFTER: TimeDelta = TimeDelta::hours(6);

/// The most bytes of failing checks a hold keeps.
const MAX_FAILED_BYTES: usize = 60_000;

/// The base branch Runs merge to, to watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainWatch {
    /// The repository's `https://github.com/owner/name` URL.
    pub repo_url: String,
    /// The branch Runs merge to.
    pub base_branch: String,
    /// The Run that started the watch: as its owner the watch reads GitHub
    /// when no merge to the base is recorded.
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
    /// Every required check that ran on the head passed, and the head's
    /// other checks finished: the base holds nothing.
    Green,
    /// A required check failed: the base is held.
    Red,
    /// A required check, or another check on the head, has not finished;
    /// GitHub could not be read; or a merge landed after the read.
    Pending,
    /// The checks the base requires could not be read, or name none: the
    /// base is not held, and its Runs say so.
    Unread,
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
    /// Whether the base's latest merge is older than [`IDLE_AFTER`], or
    /// none is recorded.
    pub idle: bool,
}

impl MainLook {
    /// Whether the watch may end after this look: nothing holds the base,
    /// and its checks settled, or no merge is recent enough to wait for.
    pub fn ends_watch(&self) -> bool {
        if self.held {
            return false;
        }
        match self.state {
            MainState::Green | MainState::Unread | MainState::Gone => true,
            MainState::Pending => self.idle,
            MainState::Red => false,
        }
    }
}

/// The task a hold files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoldTask {
    /// Its name.
    pub title: String,
    /// Its text, as markdown.
    pub markdown: String,
}

/// Whether every check on the head finished: no run, status or suite with a
/// run waits, no app holds the head, and every list was read in full.
fn head_settled(checks: &HeadChecks) -> bool {
    let waiting = |state: &CheckState| *state == CheckState::Waiting;
    !checks.truncated
        && checks.held_apps.is_empty()
        && !checks.runs.iter().any(|run| waiting(&run.state))
        && !checks.statuses.iter().any(|status| waiting(&status.state))
        && !checks
            .suites
            .iter()
            .any(|suite| suite.check_runs > 0 && waiting(&suite.state))
}

/// What the checks on a base branch's head say of the checks it
/// `required`, which name at least one.
///
/// A required check is red when it failed and no run or status of it
/// passed: a re-run that passes clears it. One that has not finished keeps
/// the base pending. One with no run or status on the head at all, as a
/// check that runs only on pull requests, does not apply once everything
/// else on the head finished; until then it keeps the base pending.
pub(crate) fn base_verdict(checks: &HeadChecks, required: &[RequiredCheck]) -> Verdict {
    let mut failed = Vec::new();
    let mut running = false;
    let mut absent = false;
    for check in required {
        if required_passed(check, checks) {
            continue;
        }
        let runs: Vec<_> = checks
            .runs
            .iter()
            .filter(|run| {
                run.name == check.name
                    && check.app_id.is_none_or(|app_id| run.app_id == Some(app_id))
            })
            .map(|run| (&run.state, run.name.as_str(), run.url.as_deref()))
            .collect();
        let statuses: Vec<_> = checks
            .statuses
            .iter()
            .filter(|status| check.app_id.is_none() && status.context == check.name)
            .map(|status| {
                (
                    &status.state,
                    status.context.as_str(),
                    status.url.as_deref(),
                )
            })
            .collect();
        if runs.is_empty() && statuses.is_empty() {
            absent = true;
            continue;
        }
        let mut red = false;
        for (state, name, url) in runs.into_iter().chain(statuses) {
            if let CheckState::Failed(conclusion) = state {
                red = true;
                failed.push(format!(
                    "- {name} ({conclusion}): {}",
                    url.unwrap_or("no page")
                ));
            }
        }
        if !red {
            running = true;
        }
    }

    if !failed.is_empty() {
        Verdict::Red(failed)
    } else if running || (absent && !head_settled(checks)) {
        Verdict::Pending
    } else {
        Verdict::Green(required.iter().map(|check| check.name.clone()).collect())
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

/// The watch's looks at a base branch, over the Run records, GitHub as the
/// owner of the Run that merged to it last, and the tasks a hold files.
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

    /// The Run whose owner reads GitHub and files the hold's task: the one
    /// that merged to the base last, else the one that started the watch.
    async fn acting_run(
        &self,
        watch: &MainWatch,
        latest: Option<&FactoryMerge>,
    ) -> Result<Option<Run>, StepError> {
        if let Some(merge) = latest
            && let Some(run) = self.repo.get_run(merge.run_id).await.map_err(internal)?
        {
            return Ok(Some(run));
        }
        self.repo.get_run(watch.run_id).await.map_err(internal)
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

    /// A look that could not read GitHub: the base stays as it was.
    async fn unseen(&self, base: &Repository, idle: bool) -> Result<MainLook, StepError> {
        let held = self.repo.main_hold(base).await.map_err(internal)?.is_some();
        Ok(MainLook {
            state: MainState::Pending,
            held,
            idle,
        })
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
        let latest = self.repo.latest_merge(&base).await.map_err(internal)?;
        let idle = latest
            .as_ref()
            .is_none_or(|merge| Utc::now() - merge.merged_at > IDLE_AFTER);
        let Some(run) = self.acting_run(&watch, latest.as_ref()).await? else {
            tracing::warn!("no run to read github as; the base stays as it was");
            return self.unseen(&base, idle).await;
        };

        // GitHub failing, or refusing the owner, decides nothing: the base
        // keeps its hold, and the next look reads it again.
        let required = match self
            .github
            .required_checks(&run.user_id, &repository, &base.base_branch)
            .await
        {
            Ok(required) => required,
            Err(error) => {
                tracing::warn!(?error, "the base's required checks could not be read");
                return self.unseen(&base, idle).await;
            }
        };
        let Some(required) = required.filter(|required| !required.checks.is_empty()) else {
            self.repo
                .mark_required_unread(&base)
                .await
                .map_err(internal)?;
            return Ok(MainLook {
                state: MainState::Unread,
                held: false,
                idle,
            });
        };

        // A merge recorded after this read started is not covered by what
        // it saw, so a green look counts from here.
        let seen_at = Utc::now();
        let read = match self
            .github
            .branch_checks(&run.user_id, &repository, &base.base_branch, true)
            .await
        {
            Ok(read) => read,
            Err(error) => {
                tracing::warn!(?error, "the base's checks could not be read");
                return self.unseen(&base, idle).await;
            }
        };
        let Some((head, checks)) = read else {
            let held = self
                .repo
                .main_hold(&base)
                .await
                .map_err(internal)?
                .is_some();
            return Ok(MainLook {
                state: MainState::Gone,
                held,
                idle,
            });
        };

        match base_verdict(&checks, &required.checks) {
            Verdict::Green(_) => {
                self.repo
                    .clear_main_hold(&base, seen_at)
                    .await
                    .map_err(internal)?;
                // A merge that landed after the read began is on a head this
                // look did not see.
                let merged_since = self
                    .repo
                    .latest_merge(&base)
                    .await
                    .map_err(internal)?
                    .is_some_and(|merge| merge.merged_at >= seen_at);
                let held = self
                    .repo
                    .main_hold(&base)
                    .await
                    .map_err(internal)?
                    .is_some();
                let state = if merged_since || held {
                    MainState::Pending
                } else {
                    MainState::Green
                };
                Ok(MainLook { state, held, idle })
            }
            Verdict::Red(failed) => {
                let hold = self
                    .repo
                    .hold_main(&base, &head, &failed_text(&failed), seen_at)
                    .await
                    .map_err(internal)?;
                if hold.task_id.is_none() {
                    self.file_task(&run, &base, &repository, &hold).await?;
                }
                Ok(MainLook {
                    state: MainState::Red,
                    held: true,
                    idle,
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
                    idle,
                })
            }
        }
    }
}
