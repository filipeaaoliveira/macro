//! The Postgres Run repository's rounds of checks (`factory_check_round`),
//! and the later prompts of a Step's session that run and fix them.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::models::{
    CheckRound, CheckRoundEnd, CheckRoundOutcome, CheckRoundStart, Outcome, RunId, StepKey,
};

struct CheckRoundRow {
    run_id: Uuid,
    seq: i32,
    round: i32,
    work_output: String,
    checks: Vec<String>,
    check_action_id: Uuid,
    started_at: DateTime<Utc>,
    outcome: Option<String>,
    report: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    fix_action_id: Option<Uuid>,
}

impl TryFrom<CheckRoundRow> for CheckRound {
    type Error = PgRunRepositoryErr;

    fn try_from(row: CheckRoundRow) -> Result<Self, Self::Error> {
        let end = match (row.outcome, row.report, row.ended_at) {
            (Some(outcome), Some(report), Some(ended_at)) => Some(CheckRoundEnd {
                outcome: match outcome.as_str() {
                    "passed" => CheckRoundOutcome::Passed,
                    "failed" => CheckRoundOutcome::Failed,
                    _ => return Err(PgRunRepositoryErr::UnknownCheckRoundOutcome(outcome)),
                },
                report,
                ended_at,
            }),
            (None, None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        Ok(CheckRound {
            round: u8::try_from(row.round).map_err(|_| PgRunRepositoryErr::OutOfRange(row.round))?,
            checks: row.checks,
            started_at: row.started_at,
            end,
            work_output: row.work_output,
            check_action_id: row.check_action_id,
            fix_action_id: row.fix_action_id,
        })
    }
}

impl PgRunRepository {
    /// The rounds of checks of the Runs `ids`, by Step, each Step's in order.
    pub(super) async fn runs_check_rounds(
        &self,
        ids: &[Uuid],
    ) -> Result<BTreeMap<(RunId, u16), Vec<CheckRound>>, PgRunRepositoryErr> {
        let rows = sqlx::query_as!(
            CheckRoundRow,
            r#"
            SELECT
                run_id, seq, round, work_output, checks, check_action_id, started_at,
                outcome, report, ended_at, fix_action_id
            FROM factory_check_round
            WHERE run_id = ANY($1)
            ORDER BY run_id, seq, round
            "#,
            ids,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut rounds: BTreeMap<(RunId, u16), Vec<CheckRound>> = BTreeMap::new();
        for row in rows {
            let step = (RunId::from_uuid(row.run_id), super::number(row.seq)?);
            rounds.entry(step).or_default().push(CheckRound::try_from(row)?);
        }
        Ok(rounds)
    }

    pub(super) async fn step_check_rounds(
        &self,
        key: StepKey,
    ) -> Result<Vec<CheckRound>, PgRunRepositoryErr> {
        let rows = sqlx::query_as!(
            CheckRoundRow,
            r#"
            SELECT
                run_id, seq, round, work_output, checks, check_action_id, started_at,
                outcome, report, ended_at, fix_action_id
            FROM factory_check_round
            WHERE run_id = $1 AND seq = $2
            ORDER BY round
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(CheckRound::try_from).collect()
    }

    pub(super) async fn record_turn(
        &self,
        key: StepKey,
        action_id: Uuid,
        offered: &[Outcome],
    ) -> Result<(), PgRunRepositoryErr> {
        let offered: Vec<String> = offered.iter().map(|o| o.as_str().to_owned()).collect();
        sqlx::query!(
            r#"
            UPDATE factory_step
            SET turn_action_id = $3, offered = $4,
                handed_outcome = NULL, handed_output = NULL, handed_at = NULL
            WHERE run_id = $1 AND seq = $2
                AND session_id IS NOT NULL AND ended_at IS NULL
                AND turn_action_id IS DISTINCT FROM $3
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            action_id,
            &offered,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(super) async fn insert_check_round(
        &self,
        key: StepKey,
        start: &CheckRoundStart,
    ) -> Result<(), PgRunRepositoryErr> {
        sqlx::query!(
            r#"
            INSERT INTO factory_check_round
                (run_id, seq, round, work_output, checks, check_action_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (run_id, seq, round) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(start.round),
            start.work_output,
            &start.checks,
            start.check_action_id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(super) async fn finish_check_round(
        &self,
        key: StepKey,
        round: u8,
        outcome: CheckRoundOutcome,
        report: &str,
    ) -> Result<(), PgRunRepositoryErr> {
        sqlx::query!(
            r#"
            UPDATE factory_check_round
            SET outcome = $4, report = $5, ended_at = now()
            WHERE run_id = $1 AND seq = $2 AND round = $3 AND ended_at IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(round),
            outcome.as_str(),
            report,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(super) async fn fix_check_round(
        &self,
        key: StepKey,
        round: u8,
        fix_action_id: Uuid,
    ) -> Result<Option<Uuid>, PgRunRepositoryErr> {
        let kept = sqlx::query_scalar!(
            r#"
            UPDATE factory_check_round
            SET fix_action_id = COALESCE(fix_action_id, $4)
            WHERE run_id = $1 AND seq = $2 AND round = $3 AND outcome = 'failed'
            RETURNING fix_action_id AS "fix_action_id!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(round),
            fix_action_id,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(kept)
    }
}
