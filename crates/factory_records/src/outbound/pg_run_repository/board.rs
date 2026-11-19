//! The Postgres Run repository's reads for the board
//! ([`RunBoardRepository`]).

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr, RunRow};
use crate::domain::board::{
    BoardQuery, BoardRun, BoardState, PullRequestRunsQuery, RunBoardRepository, search_pattern,
};
use crate::domain::evaluation::BENCHMARK_FLOW;
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
    definition_repo_url: Option<String>,
    definition_commit: Option<String>,
    pull_request_url: Option<String>,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    flawed_at: Option<DateTime<Utc>>,
    flawed_by_user_id: Option<String>,
    flawed_by_scorer: Option<String>,
    flaw_reason: Option<String>,
    task_name: Option<String>,
    task_live: bool,
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
            definition_repo_url: row.definition_repo_url,
            definition_commit: row.definition_commit,
            pull_request_url: row.pull_request_url,
            outcome: row.outcome,
            ended_at: row.ended_at,
            flawed_at: row.flawed_at,
            flawed_by_user_id: row.flawed_by_user_id,
            flawed_by_scorer: row.flawed_by_scorer,
            flaw_reason: row.flaw_reason,
        })?;
        Ok(BoardRun {
            run,
            state,
            task_name: row.task_name,
            task_live: row.task_live,
        })
    }
}

impl RunBoardRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip_all, fields(team = query.team_id.is_some(), state = ?query.state))]
    async fn board_runs(&self, query: &BoardQuery) -> Result<Vec<BoardRun>, Self::Err> {
        // Each owner (the person and, for their team's board, its members)
        // has their newest Runs past the cursor read in order through
        // `factory_run_user_created_idx`, or `factory_run_user_open_idx`
        // for the states of Runs that go, until each has the page's limit;
        // the owners' Runs are then merged newest first and cut to it. Where
        // a Run is reads its Gates, its base's Main Hold, its latest Step and
        // its task's name by primary key; whether that Step is at the merge
        // reads the Flow the Run kept, in its own row. A Benchmark's trials
        // are no one's work and are never read. A first page starts past the
        // latest time there is. The timeout bounds the read all the same.
        let state = query.state.map(|state| state.as_ref().to_string());
        let open_only = matches!(
            query.state,
            Some(BoardState::Running | BoardState::Waiting | BoardState::Held)
        );
        let search = query.search.as_deref().map(search_pattern);
        let created_before = query
            .after
            .map_or(DateTime::<Utc>::MAX_UTC, |cursor| cursor.created_at);
        let id_before = query
            .after
            .map_or(Uuid::max(), |cursor| cursor.id.as_uuid());
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        // A plan made for the filters given reads each owner's Runs in the
        // index's order; a generic plan, which a reused statement may get,
        // cannot tell which filters are unset, and may read and sort them
        // all.
        sqlx::query!("SET LOCAL plan_cache_mode = force_custom_plan")
            .execute(&mut *transaction)
            .await?;
        // The open states read only Runs that go, in their own query, so
        // the partial index applies.
        let rows = if open_only {
            sqlx::query_as!(
                BoardRunRow,
                r#"
            SELECT
                r.id AS "id!", r.request AS "request!", r.user_id AS "user_id!",
                r.flow AS "flow?", r.created_at AS "created_at!", r.task_id AS "task_id?",
                r.project_id AS "project_id?", r.repo_url AS "repo_url?",
                r.base_branch AS "base_branch?", r.created_in_person AS "created_in_person!",
                r.control_file_patterns AS "control_file_patterns?",
                r.control_files_team_id AS "control_files_team_id?",
                r.tampered_files AS "tampered_files?",
                r.named_control_files AS "named_control_files?",
                r.control_files_unlisted AS "control_files_unlisted!",
                r.standing_rules AS "standing_rules?",
                r.standing_rules_source AS "standing_rules_source?",
                r.standing_rules_project_id AS "standing_rules_project_id?",
                r.automation_id AS "automation_id?", r.automation_name AS "automation_name?",
                r.automation_trigger AS "automation_trigger?",
                r.definition_repo_url AS "definition_repo_url?",
                r.definition_commit AS "definition_commit?",
                r.pull_request_url AS "pull_request_url?",
                r.outcome AS "outcome?", r.ended_at AS "ended_at?",
                r.flawed_at AS "flawed_at?", r.flawed_by_user_id AS "flawed_by_user_id?",
                r.flawed_by_scorer AS "flawed_by_scorer?", r.flaw_reason AS "flaw_reason?",
                r.task_name AS "task_name?", r.task_live AS "task_live!",
                r.state AS "state!"
            FROM (
                SELECT $1::text AS user_id
                UNION
                SELECT tu.user_id FROM team_user tu WHERE tu.team_id = $2
            ) member
            CROSS JOIN LATERAL (
                SELECT
                    r.*,
                    CASE WHEN task.live THEN d.name END AS task_name,
                    task.live AS task_live,
                    place.state
                FROM factory_run r
                LEFT JOIN "Document" d ON d.id = r.task_id
                CROSS JOIN LATERAL (
                    SELECT d.id IS NOT NULL
                        AND d."deletedAt" IS NULL
                        AND EXISTS (
                            SELECT 1 FROM document_sub_type st
                            WHERE st.document_id = d.id AND st.sub_type = 'task'
                        ) AS live
                ) task
                CROSS JOIN LATERAL (
                    SELECT CASE
                        WHEN r.outcome IN ('success', 'cancelled', 'flawed') THEN 'finished'
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
                            SELECT CASE
                                WHEN r.flow_definition IS NULL
                                    THEN s.entry = ANY($3::text[])
                                ELSE EXISTS (
                                    SELECT 1
                                    FROM jsonb_array_elements(
                                        r.flow_definition -> 'entries'
                                    ) e
                                    WHERE e ->> 'id' = s.entry
                                        AND e ->> 'step' IN ('ready_to_merge', 'merge')
                                )
                            END
                            FROM factory_step s
                            WHERE s.run_id = r.id
                            ORDER BY s.seq DESC
                            LIMIT 1
                        ) THEN 'held'
                        ELSE 'running'
                    END AS state
                ) place
                WHERE r.user_id = member.user_id
                    AND r.ended_at IS NULL
                    AND (r.created_at, r.id) < ($4, $5)
                    AND ($6::text IS NULL OR place.state = $6)
                    AND ($7::text IS NULL OR r.flow = $7)
                    AND ($8::text IS NULL OR r.repo_url = $8)
                    AND ($9::uuid IS NULL OR r.project_id = $9)
                    AND (
                        $10::text IS NULL
                        OR r.request ILIKE $10
                        OR (task.live AND d.name ILIKE $10)
                    )
                    AND r.flow IS DISTINCT FROM $12
                ORDER BY r.created_at DESC, r.id DESC
                LIMIT $11
            ) r
            ORDER BY r.created_at DESC, r.id DESC
            LIMIT $11
            "#,
                query.user_id.as_ref(),
                query.team_id,
                &query.built_in_merge_entries,
                created_before,
                id_before,
                state,
                query.flow,
                query.repo_url,
                query.project_id,
                search,
                i64::from(query.limit),
                BENCHMARK_FLOW,
            )
            .fetch_all(&mut *transaction)
            .await?
        } else {
            sqlx::query_as!(
                BoardRunRow,
                r#"
            SELECT
                r.id AS "id!", r.request AS "request!", r.user_id AS "user_id!",
                r.flow AS "flow?", r.created_at AS "created_at!", r.task_id AS "task_id?",
                r.project_id AS "project_id?", r.repo_url AS "repo_url?",
                r.base_branch AS "base_branch?", r.created_in_person AS "created_in_person!",
                r.control_file_patterns AS "control_file_patterns?",
                r.control_files_team_id AS "control_files_team_id?",
                r.tampered_files AS "tampered_files?",
                r.named_control_files AS "named_control_files?",
                r.control_files_unlisted AS "control_files_unlisted!",
                r.standing_rules AS "standing_rules?",
                r.standing_rules_source AS "standing_rules_source?",
                r.standing_rules_project_id AS "standing_rules_project_id?",
                r.automation_id AS "automation_id?", r.automation_name AS "automation_name?",
                r.automation_trigger AS "automation_trigger?",
                r.definition_repo_url AS "definition_repo_url?",
                r.definition_commit AS "definition_commit?",
                r.pull_request_url AS "pull_request_url?",
                r.outcome AS "outcome?", r.ended_at AS "ended_at?",
                r.flawed_at AS "flawed_at?", r.flawed_by_user_id AS "flawed_by_user_id?",
                r.flawed_by_scorer AS "flawed_by_scorer?", r.flaw_reason AS "flaw_reason?",
                r.task_name AS "task_name?", r.task_live AS "task_live!",
                r.state AS "state!"
            FROM (
                SELECT $1::text AS user_id
                UNION
                SELECT tu.user_id FROM team_user tu WHERE tu.team_id = $2
            ) member
            CROSS JOIN LATERAL (
                SELECT
                    r.*,
                    CASE WHEN task.live THEN d.name END AS task_name,
                    task.live AS task_live,
                    place.state
                FROM factory_run r
                LEFT JOIN "Document" d ON d.id = r.task_id
                CROSS JOIN LATERAL (
                    SELECT d.id IS NOT NULL
                        AND d."deletedAt" IS NULL
                        AND EXISTS (
                            SELECT 1 FROM document_sub_type st
                            WHERE st.document_id = d.id AND st.sub_type = 'task'
                        ) AS live
                ) task
                CROSS JOIN LATERAL (
                    SELECT CASE
                        WHEN r.outcome IN ('success', 'cancelled', 'flawed') THEN 'finished'
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
                            SELECT CASE
                                WHEN r.flow_definition IS NULL
                                    THEN s.entry = ANY($3::text[])
                                ELSE EXISTS (
                                    SELECT 1
                                    FROM jsonb_array_elements(
                                        r.flow_definition -> 'entries'
                                    ) e
                                    WHERE e ->> 'id' = s.entry
                                        AND e ->> 'step' IN ('ready_to_merge', 'merge')
                                )
                            END
                            FROM factory_step s
                            WHERE s.run_id = r.id
                            ORDER BY s.seq DESC
                            LIMIT 1
                        ) THEN 'held'
                        ELSE 'running'
                    END AS state
                ) place
                WHERE r.user_id = member.user_id
                    AND (r.created_at, r.id) < ($4, $5)
                    AND ($6::text IS NULL OR place.state = $6)
                    AND ($7::text IS NULL OR r.flow = $7)
                    AND ($8::text IS NULL OR r.repo_url = $8)
                    AND ($9::uuid IS NULL OR r.project_id = $9)
                    AND (
                        $10::text IS NULL
                        OR r.request ILIKE $10
                        OR (task.live AND d.name ILIKE $10)
                    )
                    AND r.flow IS DISTINCT FROM $12
                ORDER BY r.created_at DESC, r.id DESC
                LIMIT $11
            ) r
            ORDER BY r.created_at DESC, r.id DESC
            LIMIT $11
            "#,
                query.user_id.as_ref(),
                query.team_id,
                &query.built_in_merge_entries,
                created_before,
                id_before,
                state,
                query.flow,
                query.repo_url,
                query.project_id,
                search,
                i64::from(query.limit),
                BENCHMARK_FLOW,
            )
            .fetch_all(&mut *transaction)
            .await?
        };
        transaction.commit().await?;
        rows.into_iter().map(BoardRun::try_from).collect()
    }

    #[tracing::instrument(err, skip_all, fields(team = query.team_id.is_some(), urls = query.urls.len()))]
    async fn pull_request_runs(
        &self,
        query: &PullRequestRunsQuery,
    ) -> Result<Vec<BoardRun>, Self::Err> {
        // Each pull request's Runs are read on their own, through
        // `factory_run_pull_request_url_idx`, kept when the person or a
        // member of their team owns them. Whether the caller sees a Run
        // depends only on its owner and task, so only the newest Run of each
        // owner and task can be the one shown: those are read, newest first,
        // up to the limit for each pull request, so one pull request's Runs
        // never crowd out another's. Where each is reads as the board's
        // read does: a flawed Run finished, and one at its merge by the Flow
        // it kept. A Benchmark's trials, which the board leaves out, are
        // read: a trial opens no pull request, and a page kept on one by
        // hand still guards its merge.
        let rows = sqlx::query_as!(
            BoardRunRow,
            r#"
            SELECT
                r.id AS "id!", r.request AS "request!", r.user_id AS "user_id!",
                r.flow AS "flow?", r.created_at AS "created_at!", r.task_id AS "task_id?",
                r.project_id AS "project_id?", r.repo_url AS "repo_url?",
                r.base_branch AS "base_branch?", r.created_in_person AS "created_in_person!",
                r.control_file_patterns AS "control_file_patterns?",
                r.control_files_team_id AS "control_files_team_id?",
                r.tampered_files AS "tampered_files?",
                r.named_control_files AS "named_control_files?",
                r.control_files_unlisted AS "control_files_unlisted!",
                r.standing_rules AS "standing_rules?",
                r.standing_rules_source AS "standing_rules_source?",
                r.standing_rules_project_id AS "standing_rules_project_id?",
                r.automation_id AS "automation_id?", r.automation_name AS "automation_name?",
                r.automation_trigger AS "automation_trigger?",
                r.definition_repo_url AS "definition_repo_url?",
                r.definition_commit AS "definition_commit?",
                r.pull_request_url AS "pull_request_url?",
                r.outcome AS "outcome?", r.ended_at AS "ended_at?",
                r.flawed_at AS "flawed_at?", r.flawed_by_user_id AS "flawed_by_user_id?",
                r.flawed_by_scorer AS "flawed_by_scorer?", r.flaw_reason AS "flaw_reason?",
                CASE WHEN task.live THEN d.name END AS "task_name?",
                task.live AS "task_live!",
                place.state AS "state!"
            FROM (SELECT DISTINCT url FROM unnest($3::text[]) AS asked(url)) asked
            CROSS JOIN LATERAL (
                SELECT heads.* FROM (
                    SELECT DISTINCT ON (o.user_id, o.task_id) o.*
                    FROM factory_run o
                    WHERE lower(o.pull_request_url) = asked.url
                        AND (
                            o.user_id = $1
                            OR o.user_id IN (
                                SELECT tu.user_id FROM team_user tu WHERE tu.team_id = $2
                            )
                        )
                    ORDER BY o.user_id, o.task_id, o.created_at DESC, o.id DESC
                ) heads
                ORDER BY heads.created_at DESC, heads.id DESC
                LIMIT $5
            ) r
            LEFT JOIN "Document" d ON d.id = r.task_id
            CROSS JOIN LATERAL (
                SELECT d.id IS NOT NULL
                    AND d."deletedAt" IS NULL
                    AND EXISTS (
                        SELECT 1 FROM document_sub_type st
                        WHERE st.document_id = d.id AND st.sub_type = 'task'
                    ) AS live
            ) task
            CROSS JOIN LATERAL (
                SELECT CASE
                    WHEN r.outcome IN ('success', 'cancelled', 'flawed') THEN 'finished'
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
                        SELECT CASE
                            WHEN r.flow_definition IS NULL
                                THEN s.entry = ANY($4::text[])
                            ELSE EXISTS (
                                SELECT 1
                                FROM jsonb_array_elements(r.flow_definition -> 'entries') e
                                WHERE e ->> 'id' = s.entry
                                    AND e ->> 'step' IN ('ready_to_merge', 'merge')
                            )
                        END
                        FROM factory_step s
                        WHERE s.run_id = r.id
                        ORDER BY s.seq DESC
                        LIMIT 1
                    ) THEN 'held'
                    ELSE 'running'
                END AS state
            ) place
            ORDER BY r.created_at DESC, r.id DESC
            "#,
            query.user_id.as_ref(),
            query.team_id,
            &query.urls,
            &query.built_in_merge_entries,
            i64::from(query.limit),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(BoardRun::try_from).collect()
    }
}
