//! The board's reads, and who may read a Run that is not theirs.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_session::domain::session_task::TaskDocumentId;
use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::{
    BoardCursor, BoardQuery, BoardRun as RecordedBoardRun, RunBoardRepository,
};
use factory_records::domain::flaws::FlawRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::Run;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use factory_records::domain::standing_rules::StandingRulesRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FactoryServiceImpl, internal};
use crate::domain::board::{BoardFilter, BoardNext, BoardPage, BoardRun, BoardScope};
use crate::domain::built_in_flows::BUILT_IN_FLOWS;
use crate::domain::models::FactoryError;

/// How many rounds of Runs a page reads at most, looking past Runs the
/// caller cannot see. Past five rounds of hidden Runs in a row (some 250),
/// the page ends with no next page: the older Runs behind them are not
/// listed, rather than reading on without end.
const BOARD_ROUNDS: usize = 5;

/// The fewest Runs a round reads, so a page of hidden Runs is passed in few
/// rounds.
const BOARD_BATCH_MIN: u16 = 50;

use crate::domain::ports::{
    PullRequests, RunWorkflows, StepExecutor, TaskAccess, TaskDirectory, UserDirectory,
};

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository
        + MainHoldRepository
        + ReviewRoundRepository
        + StandingRulesRepository
        + AutomationRepository
        + FlawRepository
        + RunBoardRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    /// The entries of the built-in Flows the worker serves that merge a
    /// Run's change: those that run `ready_to_merge` or `merge`. Only a Run
    /// that kept no Flow of its own is read against them; the others are
    /// read against their own.
    pub(super) fn built_in_merge_entries(&self) -> Vec<String> {
        BUILT_IN_FLOWS
            .into_iter()
            .filter_map(|flow| flow.read(|step| self.executor.serves(step)).ok())
            .flat_map(|flow| flow.entries)
            .filter(|entry| matches!(entry.step.as_str(), "ready_to_merge" | "merge"))
            .map(|entry| entry.id.to_string())
            .collect()
    }

    /// Whether `actor` may read `run`, which is not theirs: they are on its
    /// owner's team and can see the task it works on. A Run with no task
    /// stays its owner's.
    pub(super) async fn sees_teammates_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        run: &Run,
    ) -> Result<bool, FactoryError> {
        let Some(task_id) = run.task_id else {
            return Ok(false);
        };
        let actor_team = self.repo.team_of(actor).await.map_err(internal)?;
        let owner_team = self.repo.team_of(&run.user_id).await.map_err(internal)?;
        let same_team = match (actor_team, owner_team) {
            (Some(actor_team), Some(owner_team)) => actor_team.team_id == owner_team.team_id,
            _ => false,
        };
        if !same_team {
            return Ok(false);
        }
        self.sees_task(actor, task_id).await
    }

    /// Whether `actor` can see the task document `task_id`.
    async fn sees_task(
        &self,
        actor: &MacroUserIdStr<'static>,
        task_id: Uuid,
    ) -> Result<bool, FactoryError> {
        let Ok(task) = TaskDocumentId::parse(&task_id.to_string()) else {
            return Ok(false);
        };
        let access = self
            .tasks
            .task_access(actor, &task)
            .await
            .map_err(internal)?;
        Ok(access == TaskAccess::Visible)
    }

    /// A page of the board, as [`FactoryService::board`] reads it.
    ///
    /// A team member's Run on a task the caller cannot see never shapes the
    /// page: the board reads on past such Runs until it has one Run more
    /// than the page lists, so the page lists only Runs the caller sees and
    /// its `next` names the last of them, and only while a Run they see
    /// follows. A page of hidden Runs alone reads as no Run at all.
    ///
    /// [`FactoryService::board`]: crate::domain::ports::FactoryService::board
    pub(super) async fn board_page(
        &self,
        actor: &MacroUserIdStr<'static>,
        filter: BoardFilter,
    ) -> Result<BoardPage, FactoryError> {
        // A team's board reads its members' Runs; a person on no team sees
        // their own.
        let team_id = match filter.scope {
            BoardScope::Mine => None,
            BoardScope::Team => self
                .repo
                .team_of(actor)
                .await
                .map_err(internal)?
                .map(|team| team.team_id),
        };
        // A page says it stopped early only on a board narrowed by no
        // filter: under a search or a filter, the flag would tell how many
        // hidden Runs match it.
        let unfiltered = filter.state.is_none()
            && filter.flow.is_none()
            && filter.repo_url.is_none()
            && filter.project_id.is_none()
            && filter.search.is_none();
        let limit = usize::from(filter.limit);
        let batch = filter.limit.saturating_add(1).max(BOARD_BATCH_MIN);
        let mut query = BoardQuery {
            user_id: actor.clone(),
            team_id,
            state: filter.state,
            flow: filter.flow,
            repo_url: filter.repo_url,
            project_id: filter.project_id,
            search: filter.search,
            built_in_merge_entries: self.built_in_merge_entries(),
            after: filter.after.map(|after| BoardCursor {
                created_at: after.created_at,
                id: after.id,
            }),
            limit: batch,
        };

        // Read until one Run past the page is one the caller sees, the
        // Runs run out, or the rounds do: only a team's board has Runs to
        // leave out, so a person's own board takes one round.
        let mut seen: BTreeMap<Uuid, bool> = BTreeMap::new();
        let mut listed = Vec::with_capacity(limit + 1);
        // Whether the rounds ran out with Runs left to read.
        let mut stopped_early = false;
        for round in 0..BOARD_ROUNDS {
            let read = self.repo.board_runs(&query).await.map_err(internal)?;
            let exhausted = read.len() < usize::from(batch);
            let Some(last) = read.last() else {
                break;
            };
            query.after = Some(BoardCursor {
                created_at: last.run.created_at,
                id: last.run.id,
            });
            self.check_tasks(actor, &read, &mut seen).await?;
            for board_run in read {
                let visible = board_run.run.user_id == *actor
                    || board_run
                        .run
                        .task_id
                        .is_some_and(|task_id| seen.get(&task_id) == Some(&true));
                if visible {
                    listed.push(board_run);
                }
                if listed.len() > limit {
                    break;
                }
            }
            if listed.len() > limit || exhausted {
                break;
            }
            stopped_early = round + 1 == BOARD_ROUNDS;
        }

        let more = listed.len() > limit;
        listed.truncate(limit);
        let next = match listed.last() {
            Some(last) if more => Some(BoardNext {
                created_at: last.run.created_at,
                id: last.run.id,
            }),
            _ => None,
        };

        let shown: Vec<_> = listed
            .iter()
            .map(|listed| (listed.state, listed.task_name.clone()))
            .collect();
        let details = self
            .details(listed.into_iter().map(|listed| listed.run).collect())
            .await?;
        let runs = shown
            .into_iter()
            .zip(details)
            .map(|((state, task_name), detail)| BoardRun {
                state,
                task_name,
                detail,
            })
            .collect();
        Ok(BoardPage {
            runs,
            next,
            stopped_early: unfiltered && stopped_early && !more,
        })
    }

    /// Whether the caller can see each live task of a team member's Run in
    /// `read` not decided before, kept in `seen`: from the views kept a
    /// short while, else asked in one check. A Run's task that is not a
    /// live task is hidden without asking.
    async fn check_tasks(
        &self,
        actor: &MacroUserIdStr<'static>,
        read: &[RecordedBoardRun],
        seen: &mut BTreeMap<Uuid, bool>,
    ) -> Result<(), FactoryError> {
        let mut asked: Vec<Uuid> = Vec::new();
        for board_run in read {
            let Some(task_id) = board_run.run.task_id else {
                continue;
            };
            if board_run.run.user_id == *actor || seen.contains_key(&task_id) {
                continue;
            }
            if !board_run.task_live {
                seen.insert(task_id, false);
                continue;
            }
            match self.task_views.get(actor, task_id) {
                Some(visible) => {
                    seen.insert(task_id, visible);
                }
                None => asked.push(task_id),
            }
        }
        asked.sort_unstable();
        asked.dedup();
        if asked.is_empty() {
            return Ok(());
        }

        let viewable = self
            .tasks
            .viewable_documents(actor, &asked)
            .await
            .map_err(internal)?;
        for task_id in asked {
            let visible = viewable.contains(&task_id);
            self.task_views.keep(actor, task_id, visible);
            seen.insert(task_id, visible);
        }
        Ok(())
    }
}

/// How long a task view decided for the board is kept.
const TASK_VIEW_KEPT: Duration = Duration::from_secs(30);

/// The most task views kept; past it the expired ones are dropped, and if
/// none has, all of them.
const TASK_VIEWS_KEPT: usize = 10_000;

/// Whether a person can see a task, as the board last asked, each kept for
/// [`TASK_VIEW_KEPT`]: a board read again every few seconds does not ask
/// again for every Run of its team. A view that changed shows within that
/// time. Views are kept per person.
#[derive(Debug, Clone)]
pub(super) struct TaskViews {
    views: Arc<Mutex<HashMap<ViewKey, (bool, Instant)>>>,
    kept: Duration,
}

/// Whose view of which task: the person's user id and the task's.
type ViewKey = (String, Uuid);

impl Default for TaskViews {
    fn default() -> Self {
        Self::kept_for(TASK_VIEW_KEPT)
    }
}

impl TaskViews {
    /// Views kept for `kept` each.
    pub(super) fn kept_for(kept: Duration) -> Self {
        Self {
            views: Arc::default(),
            kept,
        }
    }

    fn get(&self, actor: &MacroUserIdStr<'static>, task_id: Uuid) -> Option<bool> {
        let views = self.views.lock().ok()?;
        let (visible, at) = views.get(&(actor.as_ref().to_string(), task_id))?;
        (at.elapsed() < self.kept).then_some(*visible)
    }

    fn keep(&self, actor: &MacroUserIdStr<'static>, task_id: Uuid, visible: bool) {
        let Ok(mut views) = self.views.lock() else {
            return;
        };
        if views.len() >= TASK_VIEWS_KEPT {
            views.retain(|_, (_, at)| at.elapsed() < self.kept);
        }
        if views.len() >= TASK_VIEWS_KEPT {
            views.clear();
        }
        views.insert(
            (actor.as_ref().to_string(), task_id),
            (visible, Instant::now()),
        );
    }
}
