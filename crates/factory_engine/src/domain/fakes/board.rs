//! The board's read in memory: the person's Runs and, for their team's
//! board, its members', newest first, past the cursor and up to the limit.
//! A Run is running, waiting at an open Gate, stopped or finished; the
//! Postgres repository's tests cover a held one.

use factory_records::domain::board::{
    BoardQuery, BoardRun, BoardState, PullRequestRunsQuery, RunBoardRepository, pull_request_key,
};
use factory_records::domain::models::{Run, RunOutcome};

use super::{FakeError, FakeRepo};

impl FakeRepo {
    fn board_state(&self, run: &Run) -> BoardState {
        match run.end.as_ref().map(|end| end.outcome) {
            Some(RunOutcome::Success | RunOutcome::Cancelled) => BoardState::Finished,
            Some(_) => BoardState::Stopped,
            None => {
                let waiting = self.steps.lock().expect("steps").iter().any(|(id, step)| {
                    *id == run.id && step.gate.as_ref().is_some_and(|gate| gate.answer.is_none())
                });
                if waiting {
                    BoardState::Waiting
                } else {
                    BoardState::Running
                }
            }
        }
    }

    fn on_board(&self, run: &Run, query: &BoardQuery) -> bool {
        let owner = run.user_id == query.user_id
            || query.team_id.is_some_and(|team_id| {
                self.teams
                    .lock()
                    .expect("teams")
                    .get(run.user_id.as_ref())
                    .is_some_and(|team| team.team_id == team_id)
            });
        let after = query.after.is_none_or(|cursor| {
            (run.created_at, run.id.as_uuid()) < (cursor.created_at, cursor.id.as_uuid())
        });
        let flow = query.flow.is_none() || run.flow == query.flow;
        let repository = query.repo_url.as_ref().is_none_or(|url| {
            run.repository
                .as_ref()
                .is_some_and(|repository| repository.url == *url)
        });
        let project = query.project_id.is_none() || run.project_id == query.project_id;
        let search = query
            .search
            .as_ref()
            .is_none_or(|words| run.request.to_lowercase().contains(&words.to_lowercase()));
        owner && after && flow && repository && project && search
    }
}

impl RunBoardRepository for FakeRepo {
    type Err = FakeError;

    async fn board_runs(&self, query: &BoardQuery) -> Result<Vec<BoardRun>, FakeError> {
        let runs: Vec<Run> = self.runs.lock().expect("runs").values().cloned().collect();
        let mut listed: Vec<BoardRun> = runs
            .into_iter()
            .filter(|run| self.on_board(run, query))
            .map(|run| BoardRun {
                state: self.board_state(&run),
                task_name: run.task_id.map(|task_id| format!("Task {task_id}")),
                task_live: run
                    .task_id
                    .is_some_and(|task_id| self.tasks.lock().expect("tasks").contains(&task_id)),
                run,
            })
            .filter(|listed| query.state.is_none_or(|state| state == listed.state))
            .collect();
        listed.sort_by_key(|listed| std::cmp::Reverse((listed.run.created_at, listed.run.id)));
        listed.truncate(usize::from(query.limit));
        Ok(listed)
    }

    async fn pull_request_runs(
        &self,
        query: &PullRequestRunsQuery,
    ) -> Result<Vec<BoardRun>, FakeError> {
        let board = BoardQuery {
            user_id: query.user_id.clone(),
            team_id: query.team_id,
            state: None,
            flow: None,
            repo_url: None,
            project_id: None,
            search: None,
            merge_entries: query.merge_entries.clone(),
            after: None,
            limit: u16::MAX,
        };
        let mut listed = self.board_runs(&board).await?;
        listed.retain(|listed| {
            listed
                .run
                .pull_request_url
                .as_deref()
                .is_some_and(|url| query.urls.contains(&pull_request_key(url)))
        });
        listed.truncate(usize::from(query.limit));
        Ok(listed)
    }
}
