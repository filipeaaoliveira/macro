//! Gate replies and the stand-in's answers, on `factory_gate`, and the
//! answered Gates the stand-in and the replay measure read.

#[cfg(test)]
mod test;

use macro_user_id::user_id::MacroUserIdStr;

use super::{PgRunRepository, PgRunRepositoryErr, number, percent};
use crate::domain::gate_replies::{AnsweredGate, GateReplyRepository, PastAnswer};
use crate::domain::models::{
    EntryId, GateAnswer, GateKind, Outcome, RunId, StandInAnswer, StepKey,
};

/// How many of the owner's newest Runs the stand-in looks through for
/// their past answers: the read stays on `factory_run_user_created_idx`
/// and the Runs' Gates by primary key.
const PAST_RUNS_READ: i64 = 100;

impl GateReplyRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, reply))]
    async fn ask_back(&self, key: StepKey, reply: &str) -> Result<bool, Self::Err> {
        let asked = sqlx::query!(
            r#"
            UPDATE factory_gate
            SET asked_back_reply = $3, asked_back_at = now()
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            reply,
        )
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(asked > 0)
    }

    #[tracing::instrument(err, skip(self, reply, user_id), fields(answer = %answer))]
    async fn answer_gate_from_reply(
        &self,
        key: StepKey,
        answer: &Outcome,
        reply: &str,
        user_id: &MacroUserIdStr<'static>,
        confidence: Option<u8>,
    ) -> Result<Option<GateAnswer>, Self::Err> {
        // A stand-in's answer the owner overrides in words stays recorded
        // beside theirs, as with a button.
        sqlx::query!(
            r#"
            UPDATE factory_gate
            SET answer = $3, note = $4, user_id = $5, reply_confidence = $6,
                answered_at = now()
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            answer.as_str(),
            reply,
            user_id.as_ref(),
            confidence.map(i16::from),
        )
        .execute(&self.pool)
        .await?;
        self.kept_answer(key).await
    }

    #[tracing::instrument(err, skip(self, answer), fields(answer = %answer.answer))]
    async fn stand_in(
        &self,
        key: StepKey,
        answer: &StandInAnswer,
    ) -> Result<Option<StandInAnswer>, Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_gate
            SET stand_in_answer = $3, stand_in_reason = $4, stand_in_confidence = $5,
                stand_in_until = $6
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL AND stand_in_answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            answer.answer.as_str(),
            answer.reason,
            i16::from(answer.confidence),
            answer.until,
        )
        .execute(&self.pool)
        .await?;

        // A Gate a person answered before the stand-in did keeps no
        // stand-in's answer worth going on with.
        let stored = sqlx::query!(
            r#"
            SELECT stand_in_answer, stand_in_reason, stand_in_confidence, stand_in_until,
                answer IS NULL AS "open!"
            FROM factory_gate
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        let Some(stored) = stored.filter(|stored| stored.open) else {
            return Ok(None);
        };
        match (
            stored.stand_in_answer,
            stored.stand_in_reason,
            stored.stand_in_confidence,
            stored.stand_in_until,
        ) {
            (Some(answer), Some(reason), Some(confidence), Some(until)) => {
                Ok(Some(StandInAnswer {
                    answer: Outcome::try_from(answer)?,
                    reason,
                    confidence: percent(confidence)?,
                    until,
                }))
            }
            _ => Err(PgRunRepositoryErr::PartialGate),
        }
    }

    #[tracing::instrument(err, skip(self))]
    async fn answer_gate_by_stand_in(&self, key: StepKey) -> Result<Option<GateAnswer>, Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_gate
            SET answer = stand_in_answer, by_stand_in = true, answered_at = now()
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL AND stand_in_answer IS NOT NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .execute(&self.pool)
        .await?;
        self.kept_answer(key).await
    }

    #[tracing::instrument(err, skip(self))]
    async fn withdraw_stand_in(&self, key: StepKey) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_gate
            SET stand_in_answer = NULL, stand_in_reason = NULL, stand_in_confidence = NULL,
                stand_in_until = NULL
            WHERE run_id = $1 AND seq = $2 AND answer IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self, owner), fields(entry = %entry))]
    async fn past_answers(
        &self,
        owner: &MacroUserIdStr<'static>,
        entry: &EntryId,
        of: RunId,
        limit: u16,
    ) -> Result<Vec<PastAnswer>, Self::Err> {
        // A Gate of the entry in a Run of the same Flow, never an agent's
        // question asked at it (a Step that ended `question`), nor the Gate
        // of an agent that stopped without a result (`no_result`). A Run
        // recorded before Runs kept their Flow has no past answers.
        let rows = sqlx::query!(
            r#"
            WITH runs AS (
                SELECT id, flow FROM factory_run
                WHERE user_id = $1 AND id <> $3
                ORDER BY created_at DESC, id DESC
                LIMIT $4
            )
            SELECT g.question, g.answer AS "answer!", g.note
            FROM runs r
            JOIN factory_step s ON s.run_id = r.id
            JOIN factory_gate g ON g.run_id = s.run_id AND g.seq = s.seq
            WHERE r.flow = (SELECT flow FROM factory_run WHERE id = $3)
                AND s.entry = $2 AND s.outcome IS DISTINCT FROM 'question'
                AND s.outcome IS DISTINCT FROM 'no_result'
                AND g.user_id = $1 AND g.answer IS NOT NULL AND g.agent_session_id IS NULL
            ORDER BY g.answered_at DESC
            LIMIT $5
            "#,
            owner.as_ref(),
            entry.as_str(),
            of.as_uuid(),
            PAST_RUNS_READ,
            i64::from(limit),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(PastAnswer {
                    question: row.question,
                    answer: Outcome::try_from(row.answer)?,
                    note: row.note,
                })
            })
            .collect()
    }

    // An offline read, for the replay measure: it reads every answered
    // Gate up to its limit, but the Gate of an agent that stopped without
    // a result, which asks nothing of the Flow.
    #[tracing::instrument(err, skip(self))]
    async fn answered_gates(&self, limit: u32) -> Result<Vec<AnsweredGate>, Self::Err> {
        let rows = sqlx::query!(
            r#"
            SELECT
                g.run_id,
                g.seq,
                s.entry,
                r.user_id AS owner,
                r.flow,
                r.request,
                s.outcome,
                s.output AS "output?",
                g.question,
                g.answers,
                g.answer AS "answer!",
                g.note,
                g.answered_at AS "answered_at!",
                (
                    SELECT p.output FROM factory_step p
                    WHERE p.run_id = g.run_id AND p.seq < g.seq AND p.role = 'planner'
                        AND p.output IS NOT NULL
                    ORDER BY p.seq DESC
                    LIMIT 1
                ) AS plan
            FROM factory_gate g
            JOIN factory_step s ON s.run_id = g.run_id AND s.seq = g.seq
            JOIN factory_run r ON r.id = g.run_id
            WHERE g.answer IS NOT NULL AND g.user_id IS NOT NULL AND g.rule IS NULL
                AND NOT g.by_stand_in AND g.reply_confidence IS NULL
                AND g.agent_session_id IS NULL
                AND s.outcome IS DISTINCT FROM 'no_result'
            ORDER BY g.answered_at, g.run_id, g.seq
            LIMIT $1
            "#,
            i64::from(limit),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let outcome = row.outcome.map(Outcome::try_from).transpose()?;
                Ok(AnsweredGate {
                    run_id: RunId::from_uuid(row.run_id),
                    seq: number(row.seq)?,
                    entry: EntryId::try_from(row.entry)?,
                    owner: MacroUserIdStr::try_from(row.owner.clone())
                        .map_err(|_| PgRunRepositoryErr::InvalidUserId(row.owner))?,
                    flow: row.flow,
                    request: row.request,
                    kind: GateKind::of(outcome.as_ref()),
                    question: row.question,
                    answers: row
                        .answers
                        .into_iter()
                        .map(Outcome::try_from)
                        .collect::<Result<_, _>>()?,
                    output: row.output.unwrap_or_default(),
                    plan: row.plan,
                    answer: Outcome::try_from(row.answer)?,
                    note: row.note,
                    answered_at: row.answered_at,
                })
            })
            .collect()
    }
}
