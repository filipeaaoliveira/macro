//! The Postgres Run repository's reads for the board
//! ([`RunBoardRepository`]).

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr, RunRow};
use crate::domain::board::{BoardQuery, BoardRun, BoardState, RunBoardRepository, search_pattern};
use crate::domain::models::Run;

/// A Run row with where the board finds it.
struct BoardRunRow {
    id: Uuid,
    request: String,
    user_id: String,
    flow: Option<String>,
    created_at: DateTime<Utc>,
    task_id: Option<String>,
    project_id: Option<Uuid>,
    repo_url: Option<String>,
    base_branch: Option<String>,
    created_in_person: bool,
    control_file_patterns: Option<Vec<String>>,
    control_files_team_id: Option<Uuid>,
    tampered_files: Option<Vec<String>>,
    named_control_files: Option<Vec<String>>,
    control_files_unlisted: bool,
    standing_rules: Option<Vec<String>>,
    standing_rules_source: Option<String>,
    standing_rules_project_id: Option<Uuid>,
    automation_id: Option<Uuid>,
    automation_name: Option<String>,
    automation_trigger: Option<String>,
    pull_request_url: Option<String>,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    state: String,
}

impl TryFrom<BoardRunRow> for BoardRun {
    type Error = PgRunRepositoryErr;

    fn try_from(row: BoardRunRow) -> Result<Self, Self::Error> {
        let state = row
            .state
            .parse::<BoardState>()
            .map_err(|_| PgRunRepositoryErr::UnknownBoardState(row.state.clone()))?;
        let run = Run::try_from(RunRow {
            id: row.id,
            request: row.request,
            user_id: row.user_id,
            flow: row.flow,
            created_at: row.created_at,
            task_id: row.task_id,
            project_id: row.project_id,
            repo_url: row.repo_url,
            base_branch: row.base_branch,
            created_in_person: row.created_in_person,
            control_file_patterns: row.control_file_patterns,
            control_files_team_id: row.control_files_team_id,
            tampered_files: row.tampered_files,
            named_control_files: row.named_control_files,
            control_files_unlisted: row.control_files_unlisted,
            standing_rules: row.standing_rules,
            standing_rules_source: row.standing_rules_source,
            standing_rules_project_id: row.standing_rules_project_id,
            automation_id: row.automation_id,
            automation_name: row.automation_name,
            automation_trigger: row.automation_trigger,
            pull_request_url: row.pull_request_url,
            outcome: row.outcome,
            ended_at: row.ended_at,
        })?;
        Ok(BoardRun { run, state })
    }
}

impl RunBoardRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip_all, fields(team = query.team_id.is_some(), state = ?query.state))]
    async fn board_runs(&self, query: &BoardQuery) -> Result<Vec<BoardRun>, Self::Err> {
        // The owners are the person and, for their team's board, the
        // team's members: each one's Runs come newest first through
        // `factory_run_user_created_idx`, or `factory_run_user_open_idx`
        // for the Runs that go, and the page stops at its limit past the
        // cursor. Where a Run is reads its Gates by primary key, its base's
        // Main Hold by primary key and its latest Step by primary key. The
        // timeout bounds the read all the same.
        let state = query.state.map(|state| state.as_ref().to_string());
        let open_only = matches!(
            query.state,
            Some(BoardState::Running | BoardState::Waiting | BoardState::Held)
        );
        let search = query.search.as_deref().map(search_pattern);
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let rows = sqlx::query_as!(
            BoardRunRow,
            r#"
            SELECT
                r.id, r.request, r.user_id, r.flow, r.created_at, r.task_id, r.project_id,
                r.repo_url, r.base_branch, r.created_in_person,
                r.control_file_patterns, r.control_files_team_id, r.tampered_files,
                r.named_control_files, r.control_files_unlisted,
                r.standing_rules, r.standing_rules_source, r.standing_rules_project_id,
                r.automation_id, r.automation_name, r.automation_trigger, r.pull_request_url,
                r.outcome, r.ended_at,
                place.state AS "state!"
            FROM factory_run r
            CROSS JOIN LATERAL (
                SELECT CASE
                    WHEN r.outcome IN ('success', 'cancelled') THEN 'finished'
                    WHEN r.outcome IS NOT NULL THEN 'stopped'
                    WHEN EXISTS (
                        SELECT 1 FROM factory_gate g
                        WHERE g.run_id = r.id AND g.answer IS NULL
                    ) THEN 'waiting'
                    WHEN EXISTS (
                        SELECT 1 FROM factory_main_hold h
                        WHERE h.repo_url = r.repo_url
                            AND h.base_branch = r.base_branch
                            AND h.held_since IS NOT NULL
                    ) AND (
                        SELECT s.entry FROM factory_step s
                        WHERE s.run_id = r.id
                        ORDER BY s.seq DESC
                        LIMIT 1
                    ) = ANY($3::text[]) THEN 'held'
                    ELSE 'running'
                END AS state
            ) place
            WHERE r.user_id = ANY(ARRAY(
                    SELECT $1::text
                    UNION
                    SELECT tu.user_id FROM team_user tu WHERE tu.team_id = $2
                ))
                AND (NOT $4::bool OR r.ended_at IS NULL)
                AND ($5::text IS NULL OR place.state = $5)
                AND ($6::text IS NULL OR r.flow = $6)
                AND ($7::text IS NULL OR r.repo_url = $7)
                AND ($8::uuid IS NULL OR r.project_id = $8)
                AND (
                    $9::text IS NULL
                    OR r.request ILIKE $9
                    OR EXISTS (
                        SELECT 1 FROM "Document" d
                        WHERE d.id = r.task_id AND d.name ILIKE $9
                    )
                )
                AND (
                    $10::timestamptz IS NULL
                    OR (r.created_at, r.id) < ($10, $11::uuid)
                )
            ORDER BY r.created_at DESC, r.id DESC
            LIMIT $12
            "#,
            query.user_id.as_ref(),
            query.team_id,
            &query.merge_entries,
            open_only,
            state,
            query.flow,
            query.repo_url,
            query.project_id,
            search,
            query.after.map(|cursor| cursor.created_at),
            query.after.map(|cursor| cursor.id.as_uuid()),
            i64::from(query.limit),
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(BoardRun::try_from).collect()
    }
}
