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
    DisputeRuling, Finding, FindingClass, FindingKind, FixAnswer, FixStatus, Lines, ReviewFinding,
    ReviewRound, ReviewRoundEnd, ReviewRoundFailure, ReviewRoundOutcome, ReviewRoundRepository,
    ReviewRoundStart, Ruling, number_rounds,
};

struct RoundRow {
    run_id: Uuid,
    seq: i32,
    round: i32,
    reviewers: i32,
    head_sha: Option<String>,
    diff_base: Option<String>,
    fix_diff: bool,
    failure: Option<String>,
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
            // Numbered among the Run's rounds once they are all read.
            round: 0,
            visit: number(row.round)?,
            seq: number(row.seq)?,
            reviewers: number(row.reviewers)?,
            head_sha: row.head_sha,
            diff_base: row.diff_base,
            fix_diff: row.fix_diff,
            failure: row.failure,
            started_at: row.started_at,
            end,
            findings: Vec::new(),
            answered: Vec::new(),
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
    blocks_anywhere: bool,
    kind: Option<String>,
    follow_up_task_id: Option<String>,
    follow_up_key: Option<Uuid>,
    follow_up_commented: bool,
    repeats_dropped: bool,
    fix_status: Option<String>,
    dispute: Option<String>,
    ruling: Option<String>,
    ruled_by: Option<i32>,
}

fn line(stored: i32) -> Result<u32, PgRunRepositoryErr> {
    u32::try_from(stored).map_err(|_| PgRunRepositoryErr::OutOfRange(stored))
}

fn stored_line(line: u32) -> Result<i32, PgRunRepositoryErr> {
    i32::try_from(line).map_err(|_| PgRunRepositoryErr::OutOfRange(i32::MAX))
}

/// The kept `word`, read by `read`: none when none was kept.
pub(super) fn stored_word<T>(
    word: Option<String>,
    read: fn(&str) -> Option<T>,
) -> Result<Option<T>, PgRunRepositoryErr> {
    word.map(|word| read(&word).ok_or(PgRunRepositoryErr::UnknownReviewWord(word)))
        .transpose()
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
            Some("dismissed") => Some(FindingKind::Dismissed),
            Some(other) => return Err(PgRunRepositoryErr::UnknownReviewWord(other.to_string())),
        };
        let fix_status = stored_word(row.fix_status, FixStatus::from_word)?;
        let ruling = stored_word(row.ruling, DisputeRuling::from_word)?;
        Ok(ReviewFinding {
            seq: number(row.seq)?,
            place: number(row.place)?,
            role: row.role.map(Role::try_from).transpose()?,
            path: row.path,
            lines,
            problem: row.problem,
            why: row.why,
            blocks_anywhere: row.blocks_anywhere,
            repeats_dropped: row.repeats_dropped,
            fix_status,
            dispute: row.dispute,
            ruling,
            ruled_by: row.ruled_by.map(number).transpose()?,
            kind,
            follow_up_task_id: row
                .follow_up_task_id
                .map(|id| Uuid::parse_str(&id).map_err(|_| PgRunRepositoryErr::InvalidTaskId(id)))
                .transpose()?,
            follow_up_key: row.follow_up_key,
            follow_up_commented: row.follow_up_commented,
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
                run_id, seq, round, reviewers, head_sha, diff_base, fix_diff, failure,
                started_at, outcome, ended_at
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
                f.problem, f.why, f.blocks_anywhere, f.kind, f.follow_up_task_id, f.follow_up_key,
                f.follow_up_commented, f.repeats_dropped, f.fix_status, f.dispute, f.ruling,
                f.ruled_by
            FROM factory_review_finding f
            JOIN factory_step s ON s.run_id = f.run_id AND s.seq = f.seq
            WHERE f.run_id = ANY($1)
            ORDER BY f.run_id, f.round_seq, f.seq, f.place
            "#,
            ids,
        )
        .fetch_all(&self.pool)
        .await?;
        let answers = sqlx::query!(
            r#"
            SELECT run_id, round_seq, seq
            FROM factory_reviewer_answer
            WHERE run_id = ANY($1) AND read_at IS NOT NULL
            ORDER BY run_id, round_seq, seq
            "#,
            ids,
        )
        .fetch_all(&self.pool)
        .await?;
        let mut answered: BTreeMap<(RunId, u16), Vec<u16>> = BTreeMap::new();
        for answer in answers {
            let round = (RunId::from_uuid(answer.run_id), number(answer.round_seq)?);
            answered.entry(round).or_default().push(number(answer.seq)?);
        }

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
            round.answered = answered.remove(&(run_id, round.seq)).unwrap_or_default();
            rounds.entry(run_id).or_default().push(round);
        }
        for run_rounds in rounds.values_mut() {
            number_rounds(run_rounds);
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

    #[tracing::instrument(err, skip(self, failure), fields(seq = failure.seq, round = failure.round))]
    async fn fail_review_round(
        &self,
        run_id: RunId,
        failure: &ReviewRoundFailure,
    ) -> Result<ReviewRound, Self::Err> {
        sqlx::query!(
            r#"
            INSERT INTO factory_review_round
                (run_id, seq, round, reviewers, fix_diff, failure, outcome, ended_at)
            VALUES ($1, $2, $3, $4, false, $5, 'failed', now())
            ON CONFLICT (run_id, seq) DO NOTHING
            "#,
            run_id.as_uuid(),
            i32::from(failure.seq),
            i32::from(failure.round),
            i32::from(failure.reviewers),
            failure.failure,
        )
        .execute(&self.pool)
        .await?;

        self.round_at(run_id, failure.seq)
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

    #[tracing::instrument(err, skip(self, findings, rulings), fields(run_id = %key.run_id, seq = key.seq, findings = findings.len(), rulings = rulings.len()))]
    async fn record_answer(
        &self,
        key: StepKey,
        round_seq: u16,
        findings: &[Finding],
        rulings: &[Ruling],
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
        let mut anywhere = Vec::with_capacity(findings.len());
        let mut repeats = Vec::with_capacity(findings.len());
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
            anywhere.push(finding.blocks_anywhere);
            repeats.push(finding.repeats_dropped);
        }
        sqlx::query!(
            r#"
            INSERT INTO factory_review_finding
                (run_id, seq, place, round_seq, path, start_line, end_line, problem, why,
                 blocks_anywhere, repeats_dropped)
            SELECT
                $1, $2, f.place, $3, f.path, f.start_line, f.end_line, f.problem, f.why,
                f.blocks_anywhere, f.repeats_dropped
            FROM UNNEST(
                $4::int[], $5::text[], $6::int[], $7::int[], $8::text[], $9::text[], $10::bool[],
                $11::bool[]
            ) AS f(place, path, start_line, end_line, problem, why, blocks_anywhere, repeats_dropped)
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
            &anywhere,
            &repeats,
        )
        .execute(&mut *transaction)
        .await?;

        // The Step's rulings replace the ones an earlier attempt made; a
        // dispute another Step ruled on keeps its ruling.
        sqlx::query!(
            r#"
            UPDATE factory_review_finding
            SET ruling = NULL, ruled_by = NULL
            WHERE run_id = $1 AND ruled_by = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .execute(&mut *transaction)
        .await?;
        let ruled_seqs: Vec<i32> = rulings.iter().map(|ruling| i32::from(ruling.seq)).collect();
        let ruled_places: Vec<i32> = rulings
            .iter()
            .map(|ruling| i32::from(ruling.place))
            .collect();
        let words: Vec<String> = rulings
            .iter()
            .map(|ruling| ruling.ruling.as_str().to_string())
            .collect();
        sqlx::query!(
            r#"
            UPDATE factory_review_finding f
            SET ruling = r.ruling, ruled_by = $2
            FROM UNNEST($3::int[], $4::int[], $5::text[]) AS r(seq, place, ruling)
            WHERE f.run_id = $1
              AND f.seq = r.seq
              AND f.place = r.place
              AND f.fix_status = 'disputed'
              AND f.ruling IS NULL
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            &ruled_seqs,
            &ruled_places,
            &words,
        )
        .execute(&mut *transaction)
        .await?;
        sqlx::query!(
            r#"
            INSERT INTO factory_reviewer_answer (run_id, seq, round_seq, read_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (run_id, seq) DO UPDATE SET read_at = now()
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(round_seq),
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(true)
    }

    #[tracing::instrument(err, skip(self, answers), fields(answers = answers.len()))]
    async fn record_fix(
        &self,
        run_id: RunId,
        round_seq: u16,
        answers: &[FixAnswer],
    ) -> Result<(), Self::Err> {
        let seqs: Vec<i32> = answers.iter().map(|answer| i32::from(answer.seq)).collect();
        let places: Vec<i32> = answers
            .iter()
            .map(|answer| i32::from(answer.place))
            .collect();
        let statuses: Vec<String> = answers
            .iter()
            .map(|answer| answer.status.as_str().to_string())
            .collect();
        let disputes: Vec<Option<String>> = answers
            .iter()
            .map(|answer| answer.dispute.clone())
            .collect();
        sqlx::query!(
            r#"
            UPDATE factory_review_finding f
            SET fix_status = a.status, dispute = a.dispute
            FROM UNNEST($3::int[], $4::int[], $5::text[], $6::text[])
                AS a(seq, place, status, dispute)
            WHERE f.run_id = $1
              AND f.round_seq = $2
              AND f.seq = a.seq
              AND f.place = a.place
              AND f.kind = 'blocker'
              AND f.ruling IS NULL
            "#,
            run_id.as_uuid(),
            i32::from(round_seq),
            &seqs,
            &places,
            &statuses,
            &disputes as &[Option<String>],
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    #[tracing::instrument(err, skip(self), fields(run_id = %key.run_id, seq = key.seq))]
    async fn reask_action(
        &self,
        key: StepKey,
        round_seq: u16,
        candidate: Uuid,
    ) -> Result<Uuid, Self::Err> {
        let kept = sqlx::query_scalar!(
            r#"
            INSERT INTO factory_reviewer_answer (run_id, seq, round_seq, reask_action_id)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (run_id, seq) DO UPDATE
            SET reask_action_id = COALESCE(factory_reviewer_answer.reask_action_id, $4)
            RETURNING reask_action_id AS "reask_action_id!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(round_seq),
            candidate,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(kept)
    }

    #[tracing::instrument(err, skip(self), fields(run_id = %key.run_id, seq = key.seq))]
    async fn reask_kept(&self, key: StepKey) -> Result<Option<Uuid>, Self::Err> {
        let kept = sqlx::query_scalar!(
            r#"
            SELECT reask_action_id
            FROM factory_reviewer_answer
            WHERE run_id = $1 AND seq = $2
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(kept.flatten())
    }

    #[tracing::instrument(err, skip(self), fields(run_id = %key.run_id, seq = key.seq))]
    async fn follow_up_key(
        &self,
        key: StepKey,
        place: u16,
        candidate: Uuid,
    ) -> Result<Option<Uuid>, Self::Err> {
        let kept = sqlx::query_scalar!(
            r#"
            UPDATE factory_review_finding
            SET follow_up_key = COALESCE(follow_up_key, $4)
            WHERE run_id = $1 AND seq = $2 AND place = $3 AND kind = 'follow_up'
            RETURNING follow_up_key AS "follow_up_key!"
            "#,
            key.run_id.as_uuid(),
            i32::from(key.seq),
            i32::from(place),
            candidate,
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(kept)
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

    #[tracing::instrument(err, skip(self), fields(run_id = %run_id, round_seq))]
    async fn mark_follow_up_commented(
        &self,
        run_id: RunId,
        round_seq: u16,
        task_id: Uuid,
    ) -> Result<(), Self::Err> {
        sqlx::query!(
            r#"
            UPDATE factory_review_finding
            SET follow_up_commented = true
            WHERE run_id = $1 AND round_seq = $2 AND follow_up_task_id = $3
            "#,
            run_id.as_uuid(),
            i32::from(round_seq),
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
