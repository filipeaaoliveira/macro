//! The Postgres Run repository's rounds of checks (`factory_check_round`),
//! and the later prompts of a Step's session that run and fix them.

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr};
use crate::domain::models::{
    CheckResult, CheckRound, CheckRoundEnd, CheckRoundOutcome, CheckRoundStart, Outcome, RunId,
    StepKey,
};

struct CheckRoundRow {
    run_id: Uuid,
    seq: i32,
    round: i32,
    work_output: String,
    checks: Vec<String>,
    commands: Vec<String>,
    declared: String,
    check_action_id: Uuid,
    started_at: DateTime<Utc>,
    outcome: Option<String>,
    report: Option<String>,
    ended_at: Option<DateTime<Utc>>,
    fix_action_id: Option<Uuid>,
    quota_ran_out: bool,
    verified: bool,
    results: Option<serde_json::Value>,
    checked_commit: Option<String>,
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
        let results: Vec<CheckResult> = match row.results {
            Some(results) => serde_json::from_value(results)
                .map_err(|_| PgRunRepositoryErr::UnreadableCheckResults)?,
            None => Vec::new(),
        };
        Ok(CheckRound {
            verified: row.verified,
            results,
            checked_commit: row.checked_commit,
            round: u8::try_from(row.round)
                .map_err(|_| PgRunRepositoryErr::OutOfRange(row.round))?,
            checks: row.checks,
            commands: row.commands,
            declared: row.declared,
            started_at: row.started_at,
            end,
            work_output: row.work_output,
            check_action_id: row.check_action_id,
            fix_action_id: row.fix_action_id,
            quota_ran_out: row.quota_ran_out,
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
                run_id, seq, round, work_output, checks, commands, declared, check_action_id,
                started_at,
                outcome, report, ended_at, fix_action_id, quota_ran_out, verified, results,
                checked_commit
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
            rounds
                .entry(step)
                .or_default()
                .push(CheckRound::try_from(row)?);
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
                run_id, seq, round, work_output, checks, commands, declared, check_action_id,
                started_at,
                outcome, report, ended_at, fix_action_id, quota_ran_out, verified, results,
                checked_commit
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
        session_id: Uuid,
        action_id: Uuid,
        place: u16,
        offered: &[Outcome],
    ) -> Result<bool, PgRunRepositoryErr> {
        let offered: Vec<String> = offered.iter().map(|o| o.as_str().to_owned()).collect();
        // Only a prompt at a later place than the one recorded is recorded,
        // so one a stale attempt sends never takes the Step back to it: the
        // place is the Step's, not the prompt's id, which the worker that
        // made it made on its own clock. Recorded now, or by an earlier
        // attempt: the outer read sees the Step as it was before the update.
        // Two attempts that record the same prompt at once may both find it
        // not recorded yet, and the one whose update then changes nothing
        // reads false: it fails as superseded and its retry goes on, which
        // costs that retry. The owner may steer the Step again while its
        // agent works on the prompt, which the Step follows from now on.
        let recorded = sqlx::query_scalar!(
            r#"
            WITH turned AS (
                UPDATE factory_step
                SET turn_action_id = $4, turn_place = $6, offered = $5,
                    handed_outcome = NULL, handed_output = NULL, handed_at = NULL,
                    steer_action_id = NULL, steer_closed = false
                WHERE run_id = $1 AND seq = $2
                    AND session_id = $3 AND ended_at IS NULL
                    AND (turn_place IS NULL OR turn_place < $6)
                RETURNING 1
            )
            SELECT EXISTS (SELECT 1 FROM turned)
                OR EXISTS (
                    SELECT 1
                    FROM factory_step
                    WHERE run_id = $1 AND seq = $2
                        AND session_id = $3 AND ended_at IS NULL
                        AND turn_action_id = $4
                ) AS "recorded!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            session_id,
            action_id,
            &offered,
            i32::from(place),
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(recorded)
    }

    pub(super) async fn insert_check_round(
        &self,
        key: StepKey,
        start: &CheckRoundStart,
    ) -> Result<(), PgRunRepositoryErr> {
        sqlx::query!(
            r#"
            INSERT INTO factory_check_round
                (run_id, seq, round, work_output, checks, commands, declared, check_action_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (run_id, seq, round) DO NOTHING
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(start.round),
            start.work_output,
            &start.checks,
            &start.commands,
            start.declared,
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

    pub(super) async fn add_check_result(
        &self,
        key: StepKey,
        round: u8,
        commit: &str,
        result: &CheckResult,
    ) -> Result<(), PgRunRepositoryErr> {
        let added = serde_json::to_value([result])
            .map_err(|_| PgRunRepositoryErr::UnreadableCheckResults)?;
        // Appended only while the round is open, and once per check: a
        // retry that ran the check again keeps its first run.
        sqlx::query!(
            r#"
            UPDATE factory_check_round
            SET verified = true, results = COALESCE(results, '[]'::jsonb) || $4,
                checked_commit = COALESCE(checked_commit, $6)
            WHERE run_id = $1 AND seq = $2 AND round = $3 AND ended_at IS NULL
                AND NOT COALESCE(results, '[]'::jsonb) @> jsonb_build_array(jsonb_build_object('name', $5::text))
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(round),
            added,
            result.name,
            commit,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(super) async fn quota_ends_check_round(
        &self,
        key: StepKey,
        round: u8,
        report: &str,
    ) -> Result<(), PgRunRepositoryErr> {
        // An open round ends failed with `report`; one that already failed
        // keeps its end. A round that passed is left alone.
        sqlx::query!(
            r#"
            UPDATE factory_check_round
            SET outcome = COALESCE(outcome, 'failed'),
                report = COALESCE(report, $4),
                ended_at = COALESCE(ended_at, now()),
                quota_ran_out = true
            WHERE run_id = $1 AND seq = $2 AND round = $3
                AND outcome IS DISTINCT FROM 'passed'
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(round),
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
