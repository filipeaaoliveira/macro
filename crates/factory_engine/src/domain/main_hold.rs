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

use chrono::{DateTime, TimeDelta, Utc};
use factory_records::domain::main_hold::{
    FactoryMerge, MainHold, MainHoldRepository, RequiredChecksState,
};
use factory_records::domain::models::{Repository, Run, RunId};
use factory_records::domain::ports::RunRepository;
use github::domain::models::MacroTaskId;
use serde::{Deserialize, Serialize};

use crate::domain::models::{
    CheckState, CodeRepository, HeadChecks, RequiredCheck, RequiredChecks, StepError,
};
use crate::domain::ports::{HoldTasks, MainWatchService, PullRequests, Retryable};
use crate::domain::pull_request_steps::{EMPTY_SUITE_GRACE, Verdict, required_passed};

/// The most suspect merges a hold's task names: the newest.
pub const SUSPECTS_LISTED: u16 = 50;

/// How long after the base's latest merge a watch that holds nothing stops:
/// long enough for slow checks on the merge to finish.
pub const IDLE_AFTER: TimeDelta = TimeDelta::hours(6);

/// How long after the base's latest merge a required check that has no run
/// or status on the head still may get one, as from a CI outside GitHub
/// that posts late: until then it keeps the base pending.
pub const REQUIRED_GRACE: TimeDelta = TimeDelta::minutes(20);

/// The most merges a look reads, and whose owners it tries to read GitHub
/// as when another's link no longer works.
const MERGES_READ: u16 = 10;

/// The passage of a hold's task a comment on it quotes.
const FAILING_CHECKS_HEADING: &str = "Failing checks:";

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
    /// The checks the base requires could not be read with nothing held,
    /// or the base requires none: it is not held, and its Runs say which.
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

/// Whether every check on the head finished at `now`: something was
/// reported; no run, status or suite with a run waits; no suite GitHub made
/// in the last [`EMPTY_SUITE_GRACE`] is still empty, as a fresh head's are
/// before their runs appear; no app holds the head; and every list was
/// read in full.
fn head_settled(checks: &HeadChecks, now: DateTime<Utc>) -> bool {
    let waiting = |state: &CheckState| *state == CheckState::Waiting;
    let reported =
        !checks.suites.is_empty() || !checks.statuses.is_empty() || !checks.runs.is_empty();
    let young_empty = checks.suites.iter().any(|suite| {
        suite.check_runs == 0
            && suite
                .created_at
                .is_none_or(|created_at| now - created_at < EMPTY_SUITE_GRACE)
    });
    reported
        && !young_empty
        && !checks.truncated
        && checks.held_apps.is_empty()
        && !checks.runs.iter().any(|run| waiting(&run.state))
        && !checks.statuses.iter().any(|status| waiting(&status.state))
        && !checks
            .suites
            .iter()
            .any(|suite| suite.check_runs > 0 && waiting(&suite.state))
}

/// What the checks on a base branch's head say at `now` of the checks it
/// `required`, which name at least one.
///
/// A required check is red when it failed and no run or status of it
/// passed: a re-run that passes clears it. One that has not finished keeps
/// the base pending. One with no run or status on the head at all, as a
/// check that runs only on pull requests, does not apply once everything
/// else on the head finished and `absent_after` passed; until then it keeps
/// the base pending, as a fresh head's checks that GitHub has not made yet,
/// or a status a CI outside GitHub posts late, would.
pub(crate) fn base_verdict(
    checks: &HeadChecks,
    required: &[RequiredCheck],
    now: DateTime<Utc>,
    absent_after: Option<DateTime<Utc>>,
) -> Verdict {
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
    } else if running
        || (absent && (!head_settled(checks, now) || absent_after.is_some_and(|after| now < after)))
    {
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
         once they do, re-runs included.\n\n{heading}\n\n{failed}\n\n",
        heading = FAILING_CHECKS_HEADING,
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

    /// The Runs whose owners a look may read GitHub as, each owner once:
    /// those of the base's recent merges, newest first, then the Run that
    /// last asked the watch to look, as a held merge does.
    async fn acting_runs(
        &self,
        watch: &MainWatch,
        recent: &[FactoryMerge],
    ) -> Result<Vec<Run>, StepError> {
        let mut runs: Vec<Run> = Vec::new();
        let ids = recent
            .iter()
            .map(|merge| merge.run_id)
            .chain(std::iter::once(watch.run_id));
        for id in ids {
            let Some(run) = self.repo.get_run(id).await.map_err(internal)? else {
                continue;
            };
            if runs.iter().all(|known| known.user_id != run.user_id) {
                runs.push(run);
            }
        }
        Ok(runs)
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
            .claim_hold_task(base, hold.since, Utc::now())
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

    /// The base requires no checks, as GitHub says in full: nothing holds
    /// a merge to it. A hold set before is lifted, and its task says why.
    async fn none_required(
        &self,
        run: &Run,
        base: &Repository,
        idle: bool,
    ) -> Result<MainLook, StepError> {
        if let Some(task_id) = self
            .repo
            .main_hold(base)
            .await
            .map_err(internal)?
            .and_then(|hold| hold.task_id)
        {
            let note = format!(
                "The factory lifted this hold: `{}` no longer requires any checks, so \
                 nothing holds merges to it.",
                base.base_branch
            );
            // The note is a courtesy: a task edited past its quote, or a
            // comment refused, never keeps a hold that no rule backs.
            if let Err(error) = self
                .tasks
                .comment(&run.user_id, task_id, FAILING_CHECKS_HEADING, &note)
                .await
            {
                tracing::warn!(?error, "the lifted hold's task was not told why");
            }
        }
        self.repo
            .mark_required(base, RequiredChecksState::None)
            .await
            .map_err(internal)?;
        Ok(MainLook {
            state: MainState::Unread,
            held: false,
            idle,
        })
    }

    /// No owner could tell which checks the base requires: a hold set before
    /// stands, as the red look that set it read them; with none, the base
    /// is marked unread.
    async fn required_unread(&self, base: &Repository, idle: bool) -> Result<MainLook, StepError> {
        if self.repo.main_hold(base).await.map_err(internal)?.is_some() {
            return Ok(MainLook {
                state: MainState::Pending,
                held: true,
                idle,
            });
        }
        self.repo
            .mark_required(base, RequiredChecksState::Unread)
            .await
            .map_err(internal)?;
        Ok(MainLook {
            state: MainState::Unread,
            held: false,
            idle,
        })
    }

    /// What the checks on the base's head say, read as `run`'s owner.
    async fn judge(
        &self,
        run: &Run,
        base: &Repository,
        repository: &CodeRepository,
        read: BaseRead,
        recent: &[FactoryMerge],
        idle: bool,
    ) -> Result<MainLook, StepError> {
        let BaseRead {
            required,
            head,
            checks,
            seen_at,
        } = read;
        let absent_after = recent.first().map(|merge| merge.merged_at + REQUIRED_GRACE);
        match base_verdict(&checks, &required.checks, Utc::now(), absent_after) {
            Verdict::Green(_) => {
                self.repo
                    .clear_main_hold(base, seen_at)
                    .await
                    .map_err(internal)?;
                // A merge recorded after the read began may not be on the
                // head it saw: GitHub merged it before it was recorded, so a
                // read that began later holds it.
                let merged_since = self
                    .repo
                    .recent_merges(base, 1)
                    .await
                    .map_err(internal)?
                    .first()
                    .is_some_and(|merge| merge.merged_at >= seen_at);
                let held = self.repo.main_hold(base).await.map_err(internal)?.is_some();
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
                    .hold_main(base, &head, &failed_text(&failed), seen_at)
                    .await
                    .map_err(internal)?;
                if hold.task_id.is_none() {
                    self.file_task(run, base, repository, &hold).await?;
                }
                Ok(MainLook {
                    state: MainState::Red,
                    held: true,
                    idle,
                })
            }
            Verdict::Pending | Verdict::Closed => self.unseen(base, idle).await,
        }
    }
}

/// What one owner read of a base branch whose required checks name some.
struct BaseRead {
    required: RequiredChecks,
    head: String,
    checks: HeadChecks,
    seen_at: DateTime<Utc>,
}

/// What reading a base branch as one owner came to.
enum OwnerRead {
    /// The base's required checks and the checks on its head.
    Read(BaseRead),
    /// The base requires no checks, as read in full.
    NoneRequired,
    /// This owner cannot tell which checks the base requires.
    Unread,
    /// GitHub refused this owner: another may read it.
    Refused,
    /// The branch is gone for this owner.
    Gone,
}

impl<R, G, T> MainWatchServiceImpl<R, G, T>
where
    R: RunRepository + MainHoldRepository,
    G: PullRequests,
    T: HoldTasks,
{
    /// Read the base as `run`'s owner. GitHub failing, as against
    /// refusing the owner, is the caller's to give up on.
    async fn read_as(
        &self,
        run: &Run,
        base: &Repository,
        repository: &CodeRepository,
    ) -> Result<OwnerRead, G::Err> {
        let required = match self
            .github
            .required_checks(&run.user_id, repository, &base.base_branch)
            .await
        {
            Ok(required) => required,
            Err(error) if !error.retryable() => return Ok(OwnerRead::Refused),
            Err(error) => return Err(error),
        };
        let required = match required {
            None => return Ok(OwnerRead::Unread),
            Some(required) if required.checks.is_empty() => {
                return Ok(if required.complete {
                    OwnerRead::NoneRequired
                } else {
                    OwnerRead::Unread
                });
            }
            Some(required) => required,
        };
        // A merge recorded after this read started is not covered by what
        // it saw, so a green look counts from here.
        let seen_at = Utc::now();
        match self
            .github
            .branch_checks(&run.user_id, repository, &base.base_branch, true)
            .await
        {
            Ok(Some((head, checks))) => Ok(OwnerRead::Read(BaseRead {
                required,
                head,
                checks,
                seen_at,
            })),
            Ok(None) => Ok(OwnerRead::Gone),
            Err(error) if !error.retryable() => Ok(OwnerRead::Refused),
            Err(error) => Err(error),
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
        let recent = self
            .repo
            .recent_merges(&base, MERGES_READ)
            .await
            .map_err(internal)?;
        let idle = recent
            .first()
            .is_none_or(|merge| Utc::now() - merge.merged_at > IDLE_AFTER);

        // Each owner in turn, until one reads the base: a link that no
        // longer works, or access lost, passes to the next.
        let mut unread = false;
        let mut gone = false;
        for run in self.acting_runs(&watch, &recent).await? {
            match self.read_as(&run, &base, &repository).await {
                Ok(OwnerRead::Read(read)) => {
                    return self
                        .judge(&run, &base, &repository, read, &recent, idle)
                        .await;
                }
                Ok(OwnerRead::NoneRequired) => {
                    return self.none_required(&run, &base, idle).await;
                }
                Ok(OwnerRead::Unread) => unread = true,
                Ok(OwnerRead::Gone) => gone = true,
                Ok(OwnerRead::Refused) => {
                    tracing::warn!(owner = %run.user_id, "github refused an owner; trying the next");
                }
                Err(error) => {
                    tracing::warn!(?error, "github could not be read; the base stays as it was");
                    return self.unseen(&base, idle).await;
                }
            }
        }
        if unread {
            return self.required_unread(&base, idle).await;
        }
        if gone {
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
        }
        tracing::warn!("no owner could read github; the base stays as it was");
        self.unseen(&base, idle).await
    }
}
