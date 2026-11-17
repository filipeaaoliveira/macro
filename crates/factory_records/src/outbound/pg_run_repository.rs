//! PostgreSQL implementation of the [`RunRepository`] port.

mod check_rounds;
mod insights;
#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::models::{
    AgentName, CheckRound, CheckRoundOutcome, CheckRoundStart, ClaimedSession, ControlFileChanges,
    EntryId, Gate, GateAnswer, HandBack, HandedBack, InvalidName, NewRun, Outcome, PassedOver,
    Repository, Role, Run, RunEnd, RunId, RunInsert, RunOutcome, SessionStep, Step, StepEnd,
    StepKey, StepSession, StoppingSession, TaskLink, TeamMembership,
};
use crate::domain::ports::RunRepository;

/// The foreign key from a Run to its task document.
const TASK_FOREIGN_KEY: &str = "factory_run_task_id_fkey";

/// The foreign key from a Gate's answer to the user who gave it.
const GATE_USER_FOREIGN_KEY: &str = "factory_gate_user_id_fkey";

/// Postgres-backed Run repository.
#[derive(Debug, Clone)]
pub struct PgRunRepository {
    pool: PgPool,
}

impl PgRunRepository {
    /// Create a repository backed by the provided pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Whether a factory Step works, or worked, in the agent session
    /// `session`. Other services ask it to tell a Step's session apart; the
    /// unique index on `session_id` serves it.
    #[tracing::instrument(err, skip(self))]
    pub async fn is_step_session(&self, session: Uuid) -> Result<bool, PgRunRepositoryErr> {
        let is_step_session = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM factory_step WHERE session_id = $1) AS "is_step_session!""#,
            session,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(is_step_session)
    }
}

/// Errors produced by the Postgres Run repository.
#[derive(Debug, thiserror::Error)]
pub enum PgRunRepositoryErr {
    /// Underlying database error.
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// A stored name breaks the rules its column checks.
    #[error(transparent)]
    InvalidName(#[from] InvalidName),
    /// A stored Run outcome is not one this code knows.
    #[error("unknown run outcome {0:?}")]
    UnknownRunOutcome(String),
    /// A stored round of checks ended with an outcome this code does not
    /// know.
    #[error("unknown check round outcome {0:?}")]
    UnknownCheckRoundOutcome(String),
    /// A stored visit or place does not fit its number.
    #[error("visit or place {0} is out of range")]
    OutOfRange(i32),
    /// A stored Run or Step has only part of its end.
    #[error("a stored end has an outcome but no time, or the other way round")]
    PartialEnd,
    /// A stored Run has a repository but no base branch, or the other way
    /// round.
    #[error("a stored run has a repository but no base branch, or the other way round")]
    PartialRepository,
    /// A stored Run has only part of the Control Files its branch changed.
    #[error("a stored run has only part of the control files its branch changed")]
    PartialControlFiles,
    /// A stored Gate has only part of its question or its answer.
    #[error("a stored gate has only part of its question or its answer")]
    PartialGate,
    /// The Step does not exist.
    #[error("the step does not exist")]
    NoSuchStep,
    /// A stored task id is not a UUID.
    #[error("stored task id {0:?} is not a UUID")]
    InvalidTaskId(String),
    /// A stored user id does not read as one.
    #[error("stored user id {0:?} does not read as one")]
    InvalidUserId(String),
    /// A stored Step has passed over a different number of Agents than it
    /// has reasons for.
    #[error("a stored step's passed over agents and their reasons do not pair up")]
    PartialPassedOver,
}

struct RunRow {
    id: Uuid,
    request: String,
    user_id: String,
    created_at: DateTime<Utc>,
    task_id: Option<String>,
    repo_url: Option<String>,
    base_branch: Option<String>,
    created_in_person: bool,
    tampered_files: Option<Vec<String>>,
    named_control_files: Option<Vec<String>>,
    control_files_unlisted: bool,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
}

impl TryFrom<RunRow> for Run {
    type Error = PgRunRepositoryErr;

    fn try_from(row: RunRow) -> Result<Self, Self::Error> {
        let end = match (row.outcome, row.ended_at) {
            (Some(outcome), Some(ended_at)) => Some(RunEnd {
                outcome: outcome
                    .parse::<RunOutcome>()
                    .map_err(|_| PgRunRepositoryErr::UnknownRunOutcome(outcome))?,
                ended_at,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        let repository = match (row.repo_url, row.base_branch) {
            (Some(url), Some(base_branch)) => Some(Repository { url, base_branch }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialRepository),
        };
        Ok(Run {
            id: RunId::from_uuid(row.id),
            request: row.request,
            user_id: MacroUserIdStr::try_from(row.user_id.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.user_id))?,
            created_at: row.created_at,
            task_id: row
                .task_id
                .map(|id| Uuid::parse_str(&id).map_err(|_| PgRunRepositoryErr::InvalidTaskId(id)))
                .transpose()?,
            repository,
            created_in_person: row.created_in_person,
            control_files: match (row.tampered_files, row.named_control_files) {
                (Some(tampered), Some(named)) => Some(ControlFileChanges {
                    tampered,
                    named,
                    unlisted: row.control_files_unlisted,
                }),
                (None, None) => None,
                _ => return Err(PgRunRepositoryErr::PartialControlFiles),
            },
            end,
        })
    }
}

struct StepRow {
    run_id: Uuid,
    seq: i32,
    entry: String,
    visit: i32,
    started_at: DateTime<Utc>,
    outcome: Option<String>,
    output: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    question: Option<String>,
    answers: Option<Vec<String>>,
    opened_at: Option<DateTime<Utc>>,
    answer: Option<String>,
    note: Option<String>,
    user_id: Option<String>,
    approved_control_files: Option<Vec<String>>,
    answered_at: Option<DateTime<Utc>>,
    role: Option<String>,
    agent: Option<String>,
    passed_over: Vec<String>,
    pass_reasons: Vec<String>,
}

impl StepRow {
    /// The Step's Gate, from the columns its left join filled.
    fn gate(&mut self) -> Result<Option<Gate>, PgRunRepositoryErr> {
        let answer = match (self.answer.take(), self.answered_at) {
            (Some(answer), Some(answered_at)) => Some(GateAnswer {
                answer: Outcome::try_from(answer)?,
                note: self.note.take(),
                user_id: self
                    .user_id
                    .take()
                    .map(|id| {
                        MacroUserIdStr::try_from(id.clone())
                            .map_err(|_| PgRunRepositoryErr::InvalidUserId(id))
                    })
                    .transpose()?,
                approved_control_files: self.approved_control_files.take(),
                answered_at,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialGate),
        };

        match (self.question.take(), self.answers.take(), self.opened_at) {
            (Some(question), Some(answers), Some(opened_at)) => Ok(Some(Gate {
                question,
                answers: answers
                    .into_iter()
                    .map(Outcome::try_from)
                    .collect::<Result<_, _>>()?,
                opened_at,
                answer,
            })),
            (None, None, None) if answer.is_none() => Ok(None),
            _ => Err(PgRunRepositoryErr::PartialGate),
        }
    }
}

/// A stored visit or place, which the schema keeps between 1 and 65535.
fn number(stored: i32) -> Result<u16, PgRunRepositoryErr> {
    u16::try_from(stored).map_err(|_| PgRunRepositoryErr::OutOfRange(stored))
}

impl TryFrom<StepRow> for Step {
    type Error = PgRunRepositoryErr;

    fn try_from(mut row: StepRow) -> Result<Self, Self::Error> {
        let gate = row.gate()?;
        let end = match (row.outcome, row.output, row.ended_at) {
            (Some(outcome), Some(output), Some(ended_at)) => Some(StepEnd {
                outcome: Outcome::try_from(outcome)?,
                output,
                ended_at,
            }),
            (None, None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        if row.passed_over.len() != row.pass_reasons.len() {
            return Err(PgRunRepositoryErr::PartialPassedOver);
        }
        let passed_over = row
            .passed_over
            .into_iter()
            .zip(row.pass_reasons)
            .map(|(agent, reason)| {
                Ok(PassedOver {
                    agent: AgentName::try_from(agent)?,
                    reason,
                })
            })
            .collect::<Result<_, InvalidName>>()?;
        Ok(Step {
            seq: number(row.seq)?,
            entry: EntryId::try_from(row.entry)?,
            visit: number(row.visit)?,
            started_at: row.started_at,
            end,
            gate,
            role: row.role.map(Role::try_from).transpose()?,
            agent: row.agent.map(AgentName::try_from).transpose()?,
            passed_over,
            checks: Vec::new(),
        })
    }
}

impl RunRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip_all, fields(id = %run.id))]
    async fn insert_run(&self, run: NewRun<'_>) -> Result<RunInsert, Self::Err> {
        let inserted = sqlx::query_as!(
            RunRow,
            r#"
            INSERT INTO factory_run
                (id, flow, request, user_id, task_id, repo_url, base_branch, created_in_person)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING
                id, request, user_id, created_at, task_id, repo_url, base_branch, created_in_person,
                tampered_files, named_control_files, control_files_unlisted, outcome, ended_at
            "#,
            run.id.as_uuid(),
            run.flow,
            run.request,
            run.user_id.as_ref(),
            run.task_id.map(|task_id| task_id.to_string()),
            run.repository.map(|repository| repository.url.as_str()),
            run.repository
                .map(|repository| repository.base_branch.as_str()),
            run.created_in_person,
        )
        .fetch_one(&self.pool)
        .await;

        match inserted {
            Ok(row) => Ok(RunInsert::Inserted(Box::new(Run::try_from(row)?))),
            Err(sqlx::Error::Database(error)) if error.constraint() == Some(TASK_FOREIGN_KEY) => {
                Ok(RunInsert::NoSuchTask)
            }
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip(self))]
    async fn link_task(&self, id: RunId, task_id: Uuid) -> Result<TaskLink, Self::Err> {
        let linked = sqlx::query!(
            "UPDATE factory_run SET task_id = $2 WHERE id = $1",
            id.as_uuid(),
            task_id.to_string(),
        )
        .execute(&self.pool)
        .await;

        match linked {
            Ok(done) if done.rows_affected() == 0 => Ok(TaskLink::NoSuchRun),
            Ok(_) => Ok(TaskLink::Linked),
            Err(sqlx::Error::Database(error)) if error.constraint() == Some(TASK_FOREIGN_KEY) => {
                Ok(TaskLink::NoSuchTask)
            }
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip(self))]
    async fn set_merge_head(&self, id: RunId, sha: &str) -> Result<(), Self::Err> {
        sqlx::query!(
            "UPDATE factory_run SET merge_head_sha = $2 WHERE id = $1",
            id.as_uuid(),
            sha,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, changes))]
    async fn set_control_file_changes(
        &self,
        id: RunId,
        changes: Option<&ControlFileChanges>,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_run
            SET tampered_files = $2, named_control_files = $3, control_files_unlisted = $4
            WHERE id = $1
            "#,
            id.as_uuid(),
            changes.map(|changes| changes.tampered.as_slice()),
            changes.map(|changes| changes.named.as_slice()),
            changes.is_some_and(|changes| changes.unlisted),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // A person is on at most one team (`team_user_user_id_unique`), and its
    // owner is on it as `owner`.
    #[tracing::instrument(err, skip(self, user_id))]
    async fn team_of(
        &self,
        user_id: &MacroUserIdStr<'static>,
    ) -> Result<Option<TeamMembership>, Self::Err> {
        let membership = sqlx::query!(
            r#"
            SELECT team_id, team_role IN ('admin', 'owner') AS "admin!"
            FROM team_user
            WHERE user_id = $1
            LIMIT 1
            "#,
            user_id.as_ref(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(membership.map(|row| TeamMembership {
            team_id: row.team_id,
            admin: row.admin,
        }))
    }

    #[tracing::instrument(err, skip(self))]
    async fn team_control_files(&self, team_id: Uuid) -> Result<Option<Vec<String>>, Self::Err> {
        let patterns = sqlx::query_scalar!(
            "SELECT control_files FROM factory_team_settings WHERE team_id = $1",
            team_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(patterns)
    }

    #[tracing::instrument(err, skip(self, patterns))]
    async fn set_team_control_files(
        &self,
        team_id: Uuid,
        patterns: Option<&[String]>,
    ) -> Result<(), Self::Err> {
        match patterns {
            Some(patterns) => {
                sqlx::query!(
                    r#"
                    INSERT INTO factory_team_settings (team_id, control_files)
                    VALUES ($1, $2)
                    ON CONFLICT (team_id)
                    DO UPDATE SET control_files = EXCLUDED.control_files, updated_at = now()
                    "#,
                    team_id,
                    patterns,
                )
                .execute(&self.pool)
                .await?;
            }
            None => {
                sqlx::query!(
                    "DELETE FROM factory_team_settings WHERE team_id = $1",
                    team_id
                )
                .execute(&self.pool)
                .await?;
            }
        }
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn merge_head(&self, id: RunId) -> Result<Option<String>, Self::Err> {
        let sha = sqlx::query_scalar!(
            "SELECT merge_head_sha FROM factory_run WHERE id = $1",
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(sha.flatten())
    }

    #[tracing::instrument(err, skip(self))]
    async fn delete_run(&self, id: RunId) -> Result<(), Self::Err> {
        sqlx::query!("DELETE FROM factory_run WHERE id = $1", id.as_uuid())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn get_run(&self, id: RunId) -> Result<Option<Run>, Self::Err> {
        let row = sqlx::query_as!(
            RunRow,
            r#"
            SELECT
                id, request, user_id, created_at, task_id, repo_url, base_branch, created_in_person,
                tampered_files, named_control_files, control_files_unlisted, outcome, ended_at
            FROM factory_run
            WHERE id = $1
            "#,
            id.as_uuid(),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(Run::try_from).transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn list_steps(&self, id: RunId) -> Result<Vec<Step>, Self::Err> {
        let rows = sqlx::query_as!(
            StepRow,
            r#"
            SELECT
                s.run_id,
                s.seq,
                s.entry,
                s.visit,
                s.started_at,
                s.outcome,
                s.output,
                s.ended_at,
                g.question AS "question?",
                g.answers AS "answers?",
                g.opened_at AS "opened_at?",
                g.answer,
                g.note,
                g.user_id,
                g.approved_control_files,
                g.answered_at,
                s.role,
                s.agent,
                s.passed_over,
                s.pass_reasons
            FROM factory_step s
            LEFT JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
            WHERE s.run_id = $1
            ORDER BY s.seq
            "#,
            id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?;
        let mut rounds = self.runs_check_rounds(&[id.as_uuid()]).await?;

        let mut steps = Vec::with_capacity(rows.len());
        for row in rows {
            let mut step = Step::try_from(row)?;
            step.checks = rounds.remove(&(id, step.seq)).unwrap_or_default();
            steps.push(step);
        }
        Ok(steps)
    }

    #[tracing::instrument(err, skip(self, user_id))]
    async fn list_task_runs(
        &self,
        task_id: Uuid,
        user_id: &MacroUserIdStr<'static>,
        limit: u16,
    ) -> Result<Vec<Run>, Self::Err> {
        // `factory_run_task_owner_idx` reads one person's Runs on the task
        // newest first and stops at the limit; the timeout bounds the read
        // all the same.
        let mut transaction = self.pool.begin().await?;
        sqlx::query!("SET LOCAL statement_timeout = '5s'")
            .execute(&mut *transaction)
            .await?;
        let rows = sqlx::query_as!(
            RunRow,
            r#"
            SELECT
                id, request, user_id, created_at, task_id, repo_url, base_branch, created_in_person,
                tampered_files, named_control_files, control_files_unlisted, outcome, ended_at
            FROM factory_run
            WHERE task_id = $1 AND user_id = $2
            ORDER BY created_at DESC, id DESC
            LIMIT $3
            "#,
            task_id.to_string(),
            user_id.as_ref(),
            i64::from(limit),
        )
        .fetch_all(&mut *transaction)
        .await?;
        transaction.commit().await?;
        rows.into_iter().map(Run::try_from).collect()
    }

    #[tracing::instrument(err, skip(self, ids), fields(runs = ids.len()))]
    async fn list_runs_steps(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<Step>>, Self::Err> {
        let ids: Vec<Uuid> = ids.iter().map(RunId::as_uuid).collect();
        let rows = sqlx::query_as!(
            StepRow,
            r#"
            SELECT
                s.run_id,
                s.seq,
                s.entry,
                s.visit,
                s.started_at,
                s.outcome,
                s.output,
                s.ended_at,
                g.question AS "question?",
                g.answers AS "answers?",
                g.opened_at AS "opened_at?",
                g.answer,
                g.note,
                g.user_id,
                g.approved_control_files,
                g.answered_at,
                s.role,
                s.agent,
                s.passed_over,
                s.pass_reasons
            FROM factory_step s
            LEFT JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
            WHERE s.run_id = ANY($1)
            ORDER BY s.run_id, s.seq
            "#,
            &ids,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut rounds = self.runs_check_rounds(&ids).await?;

        let mut steps: BTreeMap<RunId, Vec<Step>> = BTreeMap::new();
        for row in rows {
            let run_id = RunId::from_uuid(row.run_id);
            let mut step = Step::try_from(row)?;
            step.checks = rounds.remove(&(run_id, step.seq)).unwrap_or_default();
            steps.entry(run_id).or_default().push(step);
        }
        Ok(steps)
    }

    #[tracing::instrument(err, skip(self), fields(entry = %entry))]
    async fn start_step(&self, key: StepKey, entry: &EntryId, visit: u16) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_step (run_id, seq, entry, visit)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (run_id, seq) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            entry.as_str(),
            i32::from(visit),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, output), fields(outcome = %outcome))]
    async fn end_step(
        &self,
        key: StepKey,
        outcome: &Outcome,
        output: &str,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET outcome = $3, output = $4, ended_at = now()
            WHERE run_id = $1 AND seq = $2 AND ended_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            outcome.as_str(),
            output,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, question, answers))]
    async fn open_gate(
        &self,
        key: StepKey,
        question: &str,
        answers: &[Outcome],
    ) -> Result<(), Self::Err> {
        let answers: Vec<String> = answers.iter().map(|a| a.as_str().to_owned()).collect();
        sqlx::query!(
            r#"
            INSERT INTO factory_gate (run_id, seq, question, answers)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (run_id, seq) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            question,
            &answers,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, note, user_id), fields(answer = %answer))]
    async fn answer_gate(
        &self,
        key: StepKey,
        answer: &Outcome,
        note: Option<&str>,
        user_id: &MacroUserIdStr<'static>,
        approved_control_files: Option<&[String]>,
    ) -> Result<(), Self::Err> {
        // The answer is recorded once the Run's workflow took it, so an
        // account deleted since the API checked it must not stop it: the
        // answer is kept with no one as its author.
        let answered = sqlx::query!(
            r#"
            UPDATE factory_gate
            SET answer = $3, note = $4, user_id = $5, approved_control_files = $6,
                answered_at = now()
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            answer.as_str(),
            note,
            Some(user_id.as_ref()),
            approved_control_files,
        )
        .execute(&self.pool)
        .await;
        match answered {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(error))
                if error.constraint() == Some(GATE_USER_FOREIGN_KEY) =>
            {
                sqlx::query!(
                    r#"
                    UPDATE factory_gate
                    SET answer = $3, note = $4, user_id = NULL, approved_control_files = $5,
                        answered_at = now()
                    WHERE run_id = $1 AND seq = $2 AND answer IS NULL
                    "#,
                    key.run_id.as_uuid(),
                    i32::from(key.seq),
                    answer.as_str(),
                    note,
                    approved_control_files,
                )
                .execute(&self.pool)
                .await?;
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    #[tracing::instrument(err, skip(self, offered))]
    async fn claim_session(
        &self,
        key: StepKey,
        session: StepSession,
        role: &Role,
        offered: &[Outcome],
    ) -> Result<ClaimedSession, Self::Err> {
        let offered: Vec<String> = offered.iter().map(|o| o.as_str().to_owned()).collect();
        // The first attempt of a Step records its session; a retry finds it.
        let claimed = sqlx::query!(
            r#"
            UPDATE factory_step
            SET session_id = $3, action_id = $4, offered = $5, role = $6, agent = $7,
                bot_id = $8
            WHERE run_id = $1 AND seq = $2 AND session_id IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session.session_id,
            session.action_id,
            &offered,
            role.as_str(),
            session.agent.as_ref().map(AgentName::as_str),
            session.bot,
        )
        .execute(&self.pool)
        .await?;
        if claimed.rows_affected() == 1 {
            return Ok(ClaimedSession { session, new: true });
        }

        let recorded = self
            .claimed_session(key)
            .await?
            .ok_or(PgRunRepositoryErr::NoSuchStep)?;
        Ok(ClaimedSession {
            session: recorded,
            new: false,
        })
    }

    #[tracing::instrument(err, skip(self, from, to))]
    async fn pass_session(
        &self,
        key: StepKey,
        from: Uuid,
        role: &Role,
        passed: &PassedOver,
        to: Option<&StepSession>,
    ) -> Result<bool, Self::Err> {
        // A new session has not been asked to open, nor seen open, and the
        // agent handed nothing back in the one it leaves. With no next
        // session the Step keeps its own, and only the pass is recorded. A
        // session claimed before Steps recorded their Role and Agent gets
        // both, so they pair up with the Agents passed over. With no next
        // session, an Agent already recorded last is not recorded again, as
        // when overlapping attempts both find the Role exhausted. A Step
        // that took a later prompt has an Agent at work, and its checks
        // rounds go on in that session.
        let to_agent = to.and_then(|to| to.agent.as_ref()).map(AgentName::as_str);
        let recorded = sqlx::query!(
            r#"
            UPDATE factory_step s
            SET session_id = COALESCE($4, s.session_id),
                action_id = COALESCE($5, s.action_id),
                role = COALESCE(s.role, $6),
                agent = COALESCE($7, s.agent, $8),
                bot_id = CASE WHEN $4::uuid IS NULL THEN s.bot_id ELSE $10 END,
                passed_over = array_append(s.passed_over, $8),
                pass_reasons = array_append(s.pass_reasons, $9),
                prepared_at = CASE WHEN $4::uuid IS NULL THEN s.prepared_at END,
                opened_at = CASE WHEN $4::uuid IS NULL THEN s.opened_at END
            FROM factory_run r
            WHERE r.id = s.run_id AND s.run_id = $1 AND s.seq = $2 AND s.session_id = $3
                AND s.session_released_at IS NULL AND s.handed_at IS NULL
                AND s.turn_action_id IS NULL
                AND s.ended_at IS NULL AND r.ended_at IS NULL
                AND (
                    $4::uuid IS NOT NULL
                    OR s.passed_over[cardinality(s.passed_over)] IS DISTINCT FROM $8
                )
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            from,
            to.map(|to| to.session_id),
            to.map(|to| to.action_id),
            role.as_str(),
            to_agent,
            passed.agent.as_str(),
            passed.reason,
            to.and_then(|to| to.bot),
        )
        .execute(&self.pool)
        .await?;
        Ok(recorded.rows_affected() == 1)
    }

    #[tracing::instrument(err, skip(self))]
    async fn claimed_session(&self, key: StepKey) -> Result<Option<StepSession>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT session_id AS "session_id!", action_id AS "action_id!", agent, bot_id
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND session_id IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(StepSession {
                session_id: row.session_id,
                action_id: row.action_id,
                agent: row.agent.map(AgentName::try_from).transpose()?,
                bot: row.bot_id,
            })
        })
        .transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn record_prepare(
        &self,
        key: StepKey,
        session_id: Uuid,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>, Self::Err> {
        let prepared_at = sqlx::query_scalar!(
            r#"
            UPDATE factory_step s
            SET prepared_at = now()
            FROM factory_run r
            WHERE r.id = s.run_id AND s.run_id = $1 AND s.seq = $2
                AND s.session_id = $3 AND s.session_released_at IS NULL
                AND s.ended_at IS NULL AND r.ended_at IS NULL
            RETURNING s.prepared_at AS "prepared_at!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(prepared_at)
    }

    #[tracing::instrument(err, skip(self))]
    async fn forget_prepare(
        &self,
        key: StepKey,
        prepared_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET prepared_at = NULL
            WHERE run_id = $1 AND seq = $2 AND prepared_at = $3 AND opened_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            prepared_at,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn record_open(&self, key: StepKey, session_id: Uuid) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET opened_at = now()
            WHERE run_id = $1 AND seq = $2
                AND session_id = $3 AND opened_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn stopping_session(
        &self,
        key: StepKey,
        lands_within: std::time::Duration,
    ) -> Result<Option<StoppingSession>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT
                session_id AS "session_id!",
                COALESCE(turn_action_id, action_id) AS "action_id!",
                agent,
                bot_id,
                opened_at IS NOT NULL AS "opened!",
                COALESCE(prepared_at > now() - make_interval(secs => $3), false)
                    AS "may_still_open!"
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND session_id IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            lands_within.as_secs_f64(),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(StoppingSession {
                session: StepSession {
                    session_id: row.session_id,
                    action_id: row.action_id,
                    agent: row.agent.map(AgentName::try_from).transpose()?,
                    bot: row.bot_id,
                },
                opened: row.opened,
                may_still_open: row.may_still_open,
            })
        })
        .transpose()
    }

    #[tracing::instrument(err, skip(self))]
    async fn release_session(&self, key: StepKey) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET session_released_at = now()
            WHERE run_id = $1 AND seq = $2
                AND session_id IS NOT NULL AND session_released_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn holds_session(&self, key: StepKey, session_id: Uuid) -> Result<bool, Self::Err> {
        let holds = sqlx::query_scalar!(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM factory_step s
                JOIN factory_run r ON r.id = s.run_id
                WHERE s.run_id = $1 AND s.seq = $2
                    AND s.session_id = $3
                    AND s.session_released_at IS NULL
                    AND s.ended_at IS NULL
                    AND r.ended_at IS NULL
            ) AS "holds!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(holds)
    }

    #[tracing::instrument(err, skip(self))]
    async fn session_step(&self, session_id: Uuid) -> Result<Option<SessionStep>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT
                s.run_id,
                s.seq,
                r.user_id,
                s.offered AS "offered!",
                s.ended_at IS NOT NULL AS "ended!"
            FROM factory_step s
            JOIN factory_run r ON r.id = s.run_id
            WHERE s.session_id = $1
            "#,
            session_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(SessionStep {
            key: StepKey {
                run_id: RunId::from_uuid(row.run_id),
                seq: number(row.seq)?,
            },
            owner: MacroUserIdStr::try_from(row.user_id.clone())
                .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.user_id))?,
            offered: row
                .offered
                .into_iter()
                .map(Outcome::try_from)
                .collect::<Result<_, _>>()?,
            ended: row.ended,
        }))
    }

    #[tracing::instrument(err, skip(self, hand_back), fields(outcome = %hand_back.outcome))]
    async fn hand_back(&self, session_id: Uuid, hand_back: &HandBack) -> Result<bool, Self::Err> {
        let kept = sqlx::query!(
            r#"
            UPDATE factory_step
            SET handed_outcome = $2, handed_output = $3, handed_at = now()
            WHERE session_id = $1 AND ended_at IS NULL
            "#,
            session_id,
            hand_back.outcome.as_str(),
            hand_back.output,
        )
        .execute(&self.pool)
        .await?;
        Ok(kept.rows_affected() == 1)
    }

    #[tracing::instrument(err, skip(self))]
    async fn handed_back(&self, key: StepKey, action_id: Uuid) -> Result<HandedBack, Self::Err> {
        // The hand back kept is the latest prompt's: the first prompt's
        // until a later one is recorded.
        let row = sqlx::query!(
            r#"
            SELECT
                COALESCE(turn_action_id, action_id) = $3 AS "latest!",
                handed_outcome,
                handed_output
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND session_id IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            action_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        match row {
            Some(row) if !row.latest => Ok(HandedBack::Superseded),
            Some(row) => match (row.handed_outcome, row.handed_output) {
                (Some(outcome), Some(output)) => Ok(HandedBack::Handed(HandBack {
                    outcome: Outcome::try_from(outcome)?,
                    output,
                })),
                _ => Ok(HandedBack::Nothing),
            },
            // A Step with no session has no prompt yet.
            None => Ok(HandedBack::Nothing),
        }
    }

    #[tracing::instrument(err, skip(self, offered))]
    async fn begin_turn(
        &self,
        key: StepKey,
        session_id: Uuid,
        action_id: Uuid,
        place: u16,
        offered: &[Outcome],
    ) -> Result<bool, Self::Err> {
        self.record_turn(key, session_id, action_id, place, offered)
            .await
    }

    #[tracing::instrument(err, skip(self))]
    async fn check_rounds(&self, key: StepKey) -> Result<Vec<CheckRound>, Self::Err> {
        self.step_check_rounds(key).await
    }

    #[tracing::instrument(err, skip(self, start), fields(round = start.round))]
    async fn start_check_round(
        &self,
        key: StepKey,
        start: &CheckRoundStart,
    ) -> Result<(), Self::Err> {
        self.insert_check_round(key, start).await
    }

    #[tracing::instrument(err, skip(self, report))]
    async fn end_check_round(
        &self,
        key: StepKey,
        round: u8,
        outcome: CheckRoundOutcome,
        report: &str,
    ) -> Result<(), Self::Err> {
        self.finish_check_round(key, round, outcome, report).await
    }

    #[tracing::instrument(err, skip(self, report))]
    async fn end_check_round_on_quota(
        &self,
        key: StepKey,
        round: u8,
        report: &str,
    ) -> Result<(), Self::Err> {
        self.quota_ends_check_round(key, round, report).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn send_back_check_round(
        &self,
        key: StepKey,
        round: u8,
        fix_action_id: Uuid,
    ) -> Result<Option<Uuid>, Self::Err> {
        self.fix_check_round(key, round, fix_action_id).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn end_run(&self, id: RunId, outcome: RunOutcome) -> Result<(), Self::Err> {
        // The Run's end and, for a cancelled Run, its open Steps' ends land
        // together, so a Step can't end after the Run that cancelled it.
        let mut transaction = self.pool.begin().await?;
        let ended = sqlx::query!(
            r#"
            UPDATE factory_run
            SET outcome = $2, ended_at = now()
            WHERE id = $1 AND outcome IS NULL
            "#,
            id.as_uuid(),
            outcome.as_ref(),
        )
        .execute(&mut *transaction)
        .await?;
        if ended.rows_affected() == 1 && outcome == RunOutcome::Cancelled {
            sqlx::query!(
                r#"
                UPDATE factory_step
                SET outcome = 'cancelled', output = 'The run was cancelled.', ended_at = now()
                WHERE run_id = $1 AND ended_at IS NULL
                "#,
                id.as_uuid(),
            )
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}
