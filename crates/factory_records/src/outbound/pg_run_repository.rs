//! PostgreSQL implementation of the [`RunRepository`] port.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use macro_user_id::user_id::MacroUserIdStr;
use sqlx::PgPool;
use uuid::Uuid;

use crate::domain::models::{
    ClaimedSession, EntryId, Gate, GateAnswer, HandBack, InvalidName, Outcome, Run, RunEnd, RunId,
    RunInsert, RunOutcome, SessionStep, Step, StepEnd, StepKey, StepSession, TaskLink,
};
use crate::domain::ports::RunRepository;

/// The foreign key from a Run to its task document.
const TASK_FOREIGN_KEY: &str = "factory_run_task_id_fkey";

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
    /// A stored visit or place does not fit its number.
    #[error("visit or place {0} is out of range")]
    OutOfRange(i32),
    /// A stored Run or Step has only part of its end.
    #[error("a stored end has an outcome but no time, or the other way round")]
    PartialEnd,
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
}

struct RunRow {
    id: Uuid,
    request: String,
    user_id: String,
    created_at: DateTime<Utc>,
    task_id: Option<String>,
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
            end,
        })
    }
}

struct StepRow {
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
    answered_at: Option<DateTime<Utc>>,
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
        Ok(Step {
            seq: number(row.seq)?,
            entry: EntryId::try_from(row.entry)?,
            visit: number(row.visit)?,
            started_at: row.started_at,
            end,
            gate,
        })
    }
}

impl RunRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, request, user_id))]
    async fn insert_run(
        &self,
        id: RunId,
        request: &str,
        user_id: &MacroUserIdStr<'static>,
        task_id: Option<Uuid>,
    ) -> Result<RunInsert, Self::Err> {
        let inserted = sqlx::query_as!(
            RunRow,
            r#"
            INSERT INTO factory_run (id, request, user_id, task_id)
            VALUES ($1, $2, $3, $4)
            RETURNING id, request, user_id, created_at, task_id, outcome, ended_at
            "#,
            id.as_uuid(),
            request,
            user_id.as_ref(),
            task_id.map(|task_id| task_id.to_string()),
        )
        .fetch_one(&self.pool)
        .await;

        match inserted {
            Ok(row) => Ok(RunInsert::Inserted(Run::try_from(row)?)),
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
            SELECT id, request, user_id, created_at, task_id, outcome, ended_at
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
                g.answered_at
            FROM factory_step s
            LEFT JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
            WHERE s.run_id = $1
            ORDER BY s.seq
            "#,
            id.as_uuid(),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(Step::try_from).collect()
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
    ) -> Result<(), Self::Err> {
        // The answer is recorded once the Run's workflow took it, so an
        // account deleted since the API checked it must not stop it: it
        // records no one.
        sqlx::query!(
            r#"
            UPDATE factory_gate
            SET answer = $3,
                note = $4,
                user_id = (SELECT id FROM "User" WHERE id = $5),
                answered_at = now()
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            answer.as_str(),
            note,
            user_id.as_ref(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, offered))]
    async fn claim_session(
        &self,
        key: StepKey,
        session: StepSession,
        offered: &[Outcome],
    ) -> Result<ClaimedSession, Self::Err> {
        let offered: Vec<String> = offered.iter().map(|o| o.as_str().to_owned()).collect();
        // The first attempt of a Step records its session; a retry finds it.
        let claimed = sqlx::query!(
            r#"
            UPDATE factory_step
            SET session_id = $3, action_id = $4, offered = $5
            WHERE run_id = $1 AND seq = $2 AND session_id IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session.session_id,
            session.action_id,
            &offered,
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

    #[tracing::instrument(err, skip(self))]
    async fn claimed_session(&self, key: StepKey) -> Result<Option<StepSession>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT session_id AS "session_id!", action_id AS "action_id!"
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND session_id IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|row| StepSession {
            session_id: row.session_id,
            action_id: row.action_id,
        }))
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
    async fn hand_back(&self, session_id: Uuid, hand_back: &HandBack) -> Result<(), Self::Err> {
        sqlx::query!(
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
        Ok(())
    }

    #[tracing::instrument(err, skip(self))]
    async fn handed_back(&self, key: StepKey) -> Result<Option<HandBack>, Self::Err> {
        let row = sqlx::query!(
            r#"
            SELECT handed_outcome AS "outcome!", handed_output AS "output!"
            FROM factory_step
            WHERE run_id = $1 AND seq = $2 AND handed_at IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(HandBack {
                outcome: Outcome::try_from(row.outcome)?,
                output: row.output,
            })
        })
        .transpose()
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
