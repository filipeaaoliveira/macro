//! The board's reads, and who may read a Run that is not theirs.

use std::collections::BTreeMap;

use agent_session::domain::session_task::TaskDocumentId;
use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::{BoardCursor, BoardQuery, RunBoardRepository};
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

        // One Run past the page tells whether another follows. The next
        // page starts past the last Run this one read, whether it was
        // listed or left out, so a page of hidden Runs never repeats.
        let limit = filter.limit;
        let mut read = self
            .repo
            .board_runs(&BoardQuery {
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
                limit: limit.saturating_add(1),
            })
            .await
            .map_err(internal)?;
        let more = read.len() > usize::from(limit);
        read.truncate(usize::from(limit));
        let next = match read.last() {
            Some(last) if more => Some(BoardNext {
                created_at: last.run.created_at,
                id: last.run.id,
            }),
            _ => None,
        };

        // A team member's Run is listed only on a task the caller can see:
        // one with no task stays its owner's. Each task is asked once.
        let mut seen: BTreeMap<Uuid, bool> = BTreeMap::new();
        let mut listed = Vec::with_capacity(read.len());
        for board_run in read {
            let run = &board_run.run;
            let visible = if run.user_id == *actor {
                true
            } else if let Some(task_id) = run.task_id {
                match seen.get(&task_id) {
                    Some(visible) => *visible,
                    None => {
                        let visible = self.sees_task(actor, task_id).await?;
                        seen.insert(task_id, visible);
                        visible
                    }
                }
            } else {
                false
            };
            if visible {
                listed.push(board_run);
            }
        }

        let states: Vec<_> = listed.iter().map(|listed| listed.state).collect();
        let details = self
            .details(listed.into_iter().map(|listed| listed.run).collect())
            .await?;
        let runs = states
            .into_iter()
            .zip(details)
            .map(|(state, detail)| BoardRun { state, detail })
            .collect();
        Ok(BoardPage { runs, next })
    }
}
