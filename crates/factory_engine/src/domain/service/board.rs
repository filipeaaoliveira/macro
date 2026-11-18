//! The board's reads, and who may read a Run that is not theirs.

use std::collections::BTreeMap;

use agent_session::domain::session_task::TaskDocumentId;
use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::{
    BoardCursor, BoardQuery, BoardRun as RecordedBoardRun, RunBoardRepository,
};
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::Run;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::review_rounds::ReviewRoundRepository;
use factory_records::domain::standing_rules::StandingRulesRepository;
use futures::{StreamExt as _, TryStreamExt as _, stream};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FactoryServiceImpl, internal};
use crate::domain::board::{BoardFilter, BoardNext, BoardPage, BoardRun, BoardScope};
use crate::domain::built_in_flows::BUILT_IN_FLOWS;
use crate::domain::models::FactoryError;

/// How many rounds of Runs a page reads at most, looking past Runs the
/// caller cannot see.
const BOARD_ROUNDS: usize = 5;

/// The fewest Runs a round reads, so a page of hidden Runs is passed in few
/// rounds.
const BOARD_BATCH_MIN: u16 = 50;

/// How many task access checks run at once.
const TASK_CHECKS_AT_ONCE: usize = 8;
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
        + RunBoardRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    /// The entries of the built-in Flows the worker serves that merge a
    /// Run's change: those that run `ready_to_merge` or `merge`.
    pub(super) fn merge_entries(&self) -> Vec<String> {
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
            merge_entries: self.merge_entries(),
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
        for _ in 0..BOARD_ROUNDS {
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
        Ok(BoardPage { runs, next })
    }

    /// Whether the caller can see each task of a team member's Run in
    /// `read` not asked before, kept in `seen`; asked at once, a few at a
    /// time.
    async fn check_tasks(
        &self,
        actor: &MacroUserIdStr<'static>,
        read: &[RecordedBoardRun],
        seen: &mut BTreeMap<Uuid, bool>,
    ) -> Result<(), FactoryError> {
        let mut asked: Vec<Uuid> = read
            .iter()
            .filter(|board_run| board_run.run.user_id != *actor)
            .filter_map(|board_run| board_run.run.task_id)
            .filter(|task_id| !seen.contains_key(task_id))
            .collect();
        asked.sort_unstable();
        asked.dedup();
        let answers: Vec<(Uuid, bool)> = stream::iter(asked)
            .map(|task_id| async move {
                self.sees_task(actor, task_id)
                    .await
                    .map(|visible| (task_id, visible))
            })
            .buffer_unordered(TASK_CHECKS_AT_ONCE)
            .try_collect()
            .await?;
        seen.extend(answers);
        Ok(())
    }
}
