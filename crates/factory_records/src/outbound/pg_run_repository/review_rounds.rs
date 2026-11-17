//! The Postgres Run repository's review rounds (`factory_review_round`) and
//! what their reviewers found (`factory_review_finding`).

#[cfg(test)]
mod test;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::{PgRunRepository, PgRunRepositoryErr, number};
use crate::domain::models::{Role, RunId, StepKey};
use crate::domain::review_rounds::{
    Finding, FindingClass, FindingKind, Lines, ReviewFinding, ReviewRound, ReviewRoundEnd,
    ReviewRoundOutcome, ReviewRoundRepository, ReviewRoundStart,
};

struct RoundRow {
    run_id: Uuid,
    seq: i32,
    round: i32,
    reviewers: i32,
    head_sha: String,
    diff_base: String,
    fix_diff: bool,
    started_at: DateTime<Utc>,
    outcome: Option<String>,
    ended_at: Option<DateTime<Utc>>,
}

impl TryFrom<RoundRow> for ReviewRound {
    type Error = PgRunRepositoryErr;

    fn try_from(row: RoundRow) -> Result<Self, Self::Error> {
        let end = match (row.outcome, row.ended_at) {
            (Some(outcome), Some(ended_at)) => Some(ReviewRoundEnd {
                outcome: match outcome.as_str() {
                    "pass" => ReviewRoundOutcome::Pass,
                    "changes" => ReviewRoundOutcome::Changes,
                    "failed" => ReviewRoundOutcome::Failed,
                    _ => return Err(PgRunRepositoryErr::UnknownReviewWord(outcome)),
                },
                ended_at,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialEnd),
        };
        Ok(ReviewRound {
            round: number(row.round)?,
            seq: number(row.seq)?,
            reviewers: number(row.reviewers)?,
            head_sha: row.head_sha,
            diff_base: row.diff_base,
            fix_diff: row.fix_diff,
            started_at: row.started_at,
            end,
            findings: Vec::new(),
        })
    }
}

struct FindingRow {
    run_id: Uuid,
    round_seq: i32,
    seq: i32,
    place: i32,
    role: Option<String>,
    path: String,
    start_line: Option<i32>,
    end_line: Option<i32>,
    problem: String,
    why: String,
    kind: Option<String>,
    follow_up_task_id: Option<String>,
}

fn line(stored: i32) -> Result<u32, PgRunRepositoryErr> {
    u32::try_from(stored).map_err(|_| PgRunRepositoryErr::OutOfRange(stored))
}

fn stored_line(line: u32) -> Result<i32, PgRunRepositoryErr> {
    i32::try_from(line).map_err(|_| PgRunRepositoryErr::OutOfRange(i32::MAX))
}

impl TryFrom<FindingRow> for ReviewFinding {
    type Error = PgRunRepositoryErr;

    fn try_from(row: FindingRow) -> Result<Self, Self::Error> {
        let lines = match (row.start_line, row.end_line) {
            (Some(start), Some(end)) => Some(Lines {
                start: line(start)?,
                end: line(end)?,
            }),
            (None, None) => None,
            _ => return Err(PgRunRepositoryErr::PartialLines),
        };
        let kind = match row.kind.as_deref() {
            None => None,
            Some("blocker") => Some(FindingKind::Blocker),
            Some("follow_up") => Some(FindingKind::FollowUp),
            Some(other) => return Err(PgRunRepositoryErr::UnknownReviewWord(other.to_string())),
        };
        Ok(ReviewFinding {
            seq: number(row.seq)?,
            place: number(row.place)?,
            role: row.role.map(Role::try_from).transpose()?,
            path: row.path,
            lines,
            problem: row.problem,
            why: row.why,
            kind,
            follow_up_task_id: row
                .follow_up_task_id
                .map(|id| Uuid::parse_str(&id).map_err(|_| PgRunRepositoryErr::InvalidTaskId(id)))
                .transpose()?,
        })
    }
}

impl PgRunRepository {
    /// The review rounds of the Runs `ids`, each with its findings.
    async fn rounds_of(
        &self,
        ids: &[Uuid],
    ) -> Result<BTreeMap<RunId, Vec<ReviewRound>>, PgRunRepositoryErr> {
        let rows = sqlx::query_as!(
            RoundRow,
            r#"
            SELECT
                run_id, seq, round, reviewers, head_sha, diff_base, fix_diff, started_at,
                outcome, ended_at
            FROM factory_review_round
            WHERE run_id = ANY($1)
            ORDER BY run_id, seq
            "#,
            ids,
        )
        .fetch_all(&self.pool)
        .await?;
        let findings = sqlx::query_as!(
            FindingRow,
            r#"
            SELECT
                f.run_id, f.round_seq, f.seq, f.place, s.role, f.path, f.start_line, f.end_line,
                f.problem, f.why, f.kind, f.follow_up_task_id
            FROM factory_review_finding f
            JOIN factory_step s ON s.run_id = f.run_id AND s.seq = f.seq
            WHERE f.run_id = ANY($1)
            ORDER BY f.run_id, f.round_seq, f.seq, f.place
            "#,
            ids,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut found: BTreeMap<(RunId, u16), Vec<ReviewFinding>> = BTreeMap::new();
        for row in findings {
            let round = (RunId::from_uuid(row.run_id), number(row.round_seq)?);
            found
                .entry(round)
                .or_default()
                .push(ReviewFinding::try_from(row)?);
        }

        let mut rounds: BTreeMap<RunId, Vec<ReviewRound>> = BTreeMap::new();
        for row in rows {
            let run_id = RunId::from_uuid(row.run_id);
            let mut round = ReviewRound::try_from(row)?;
            round.findings = found.remove(&(run_id, round.seq)).unwrap_or_default();
            rounds.entry(run_id).or_default().push(round);
        }
        Ok(rounds)
    }

    /// The Run's round that starts at `seq`, if it was recorded.
    async fn round_at(
        &self,
        run_id: RunId,
        seq: u16,
    ) -> Result<Option<ReviewRound>, PgRunRepositoryErr> {
        let rounds = self.review_rounds(run_id).await?;
        Ok(rounds.into_iter().find(|round| round.seq == seq))
    }
}

impl ReviewRoundRepository for PgRunRepository {
    type Err = PgRunRepositoryErr;

    #[tracing::instrument(err, skip(self, start), fields(seq = start.seq, round = start.round))]
    async fn start_review_round(
        &self,
        run_id: RunId,
        start: &ReviewRoundStart,
    ) -> Result<ReviewRound, Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_review_round
                (run_id, seq, round, reviewers, head_sha, diff_base, fix_diff)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (run_id, seq) DO NOTHING
            "#,
            run_id.as_uuid(),
            i32::from(start.seq),
            i32::from(start.round),
            i32::from(start.reviewers),
            start.head_sha,
            start.diff_base,
            start.fix_diff,
        )
        .execute(&self.pool)
        .await?;

        self.round_at(run_id, start.seq)
            .await?
            .ok_or(PgRunRepositoryErr::NoSuchStep)
    }

    #[tracing::instrument(err, skip(self))]
    async fn review_rounds(&self, run_id: RunId) -> Result<Vec<ReviewRound>, Self::Err> {
        let mut rounds = self.rounds_of(&[run_id.as_uuid()]).await?;
        Ok(rounds.remove(&run_id).unwrap_or_default())
    }

    #[tracing::instrument(err, skip(self, ids), fields(runs = ids.len()))]
    async fn runs_review_rounds(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<ReviewRound>>, Self::Err> {
        let ids: Vec<Uuid> = ids.iter().map(RunId::as_uuid).collect();
        self.rounds_of(&ids).await
    }

    #[tracing::instrument(err, skip(self, findings), fields(run_id = %key.run_id, seq = key.seq, findings = findings.len()))]
    async fn record_findings(
        &self,
        key: StepKey,
        round_seq: u16,
        findings: &[Finding],
    ) -> Result<bool, Self::Err> {
        // The Step's row is locked while its findings are replaced, so its
        // end, which takes the same lock, comes before or after them.
        let mut transaction = self.pool.begin().await?;
        let open = sqlx::query_scalar!(
            r#"
            SELECT ended_at IS NULL AS "open!"
            FROM factory_step
            WHERE run_id = $1 AND seq = $2
            FOR UPDATE
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&mut *transaction)
        .await?;
        if open != Some(true) {
            return Ok(false);
        }

        sqlx::query!(
            "DELETE FROM factory_review_finding WHERE run_id = $1 AND seq = $2",
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .execute(&mut *transaction)
        .await?;

        let mut places = Vec::with_capacity(findings.len());
        let mut paths = Vec::with_capacity(findings.len());
        let mut starts = Vec::with_capacity(findings.len());
        let mut ends = Vec::with_capacity(findings.len());
        let mut problems = Vec::with_capacity(findings.len());
        let mut whys = Vec::with_capacity(findings.len());
        for (place, finding) in (1_i32..).zip(findings) {
            let (start, end) = match finding.lines {
                Some(lines) => (
                    Some(stored_line(lines.start)?),
                    Some(stored_line(lines.end)?),
                ),
                None => (None, None),
            };
            places.push(place);
            paths.push(finding.path.clone());
            starts.push(start);
            ends.push(end);
            problems.push(finding.problem.clone());
            whys.push(finding.why.clone());
        }
        sqlx::query!(
            r#"
            INSERT INTO factory_review_finding
                (run_id, seq, place, round_seq, path, start_line, end_line, problem, why)
            SELECT $1, $2, f.place, $3, f.path, f.start_line, f.end_line, f.problem, f.why
            FROM UNNEST($4::int[], $5::text[], $6::int[], $7::int[], $8::text[], $9::text[])
                AS f(place, path, start_line, end_line, problem, why)
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(round_seq),
            &places,
            &paths,
            &starts as &[Option<i32>],
            &ends as &[Option<i32>],
            &problems,
            &whys,
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    #[tracing::instrument(err, skip(self, classes), fields(classes = classes.len()))]
    async fn classify_findings(
        &self,
        run_id: RunId,
        round_seq: u16,
        classes: &[FindingClass],
    ) -> Result<(), Self::Err> {
        let seqs: Vec<i32> = classes.iter().map(|class| i32::from(class.seq)).collect();
        let places: Vec<i32> = classes.iter().map(|class| i32::from(class.place)).collect();
        let kinds: Vec<String> = classes
            .iter()
            .map(|class| class.kind.as_str().to_string())
            .collect();
        sqlx::query!(
            r#"
            UPDATE factory_review_finding f
            SET kind = c.kind
            FROM UNNEST($3::int[], $4::int[], $5::text[]) AS c(seq, place, kind)
            WHERE f.run_id = $1
              AND f.round_seq = $2
              AND f.seq = c.seq
              AND f.place = c.place
              AND f.kind IS NULL
            "#,
            run_id.as_uuid(),
            i32::from(round_seq),
            &seqs,
            &places,
            &kinds,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self), fields(run_id = %key.run_id, seq = key.seq))]
    async fn link_follow_up(
        &self,
        key: StepKey,
        place: u16,
        task_id: Uuid,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_review_finding
            SET follow_up_task_id = $4
            WHERE run_id = $1
              AND seq = $2
              AND place = $3
              AND kind = 'follow_up'
              AND follow_up_task_id IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(place),
            task_id.to_string(),
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self), fields(outcome = outcome.as_str()))]
    async fn end_review_round(
        &self,
        run_id: RunId,
        seq: u16,
        outcome: ReviewRoundOutcome,
    ) -> Result<Option<ReviewRound>, Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_review_round
            SET outcome = $3, ended_at = now()
            WHERE run_id = $1 AND seq = $2 AND outcome IS NULL
            "#,
            run_id.as_uuid(),
            i32::from(seq),
            outcome.as_str(),
        )
        .execute(&self.pool)
        .await?;
        self.round_at(run_id, seq).await
    }
}
