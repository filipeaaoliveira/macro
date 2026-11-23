//! Review rounds in memory, with the same first-write-wins rules as the
//! Postgres repository.

use std::collections::BTreeMap;

use chrono::Utc;
use factory_records::domain::models::{Role, RunId, StepKey};
use factory_records::domain::review_rounds::{
    Finding, FindingClass, FindingKind, FixAnswer, FixStatus, ReviewFinding, ReviewRound,
    ReviewRoundEnd, ReviewRoundFailure, ReviewRoundOutcome, ReviewRoundRepository,
    ReviewRoundStart, Ruling, number_rounds,
};
use uuid::Uuid;

use super::{FakeError, FakeRepo};

impl FakeRepo {
    /// Give the Step `key` the Role its agent fills, as claiming its
    /// session does.
    pub(crate) fn with_step_role(&self, key: StepKey, role: Role) {
        self.with_step(key, |step| step.role = Some(role));
    }

    fn with_round<T>(
        &self,
        run_id: RunId,
        seq: u16,
        change: impl FnOnce(&mut ReviewRound) -> T,
    ) -> Option<T> {
        let mut rounds = self.review_rounds.lock().expect("rounds");
        rounds
            .iter_mut()
            .find(|(run, round)| *run == run_id && round.seq == seq)
            .map(|(_, round)| change(round))
    }

    fn rounds_of_run(&self, run_id: RunId) -> Vec<ReviewRound> {
        let mut rounds: Vec<ReviewRound> = self
            .review_rounds
            .lock()
            .expect("rounds")
            .iter()
            .filter(|(run, _)| *run == run_id)
            .map(|(_, round)| round.clone())
            .collect();
        rounds.sort_by_key(|round| round.seq);
        number_rounds(&mut rounds);
        // A finding reads with its Step's Role, as the Postgres join reads
        // it.
        for round in &mut rounds {
            for finding in &mut round.findings {
                let key = StepKey {
                    run_id,
                    seq: finding.seq,
                };
                finding.role = self.with_step(key, |step| step.role.clone()).flatten();
            }
        }
        rounds
    }
}

impl ReviewRoundRepository for FakeRepo {
    type Err = FakeError;

    async fn start_review_round(
        &self,
        run_id: RunId,
        start: &ReviewRoundStart,
    ) -> Result<ReviewRound, FakeError> {
        if self.with_round(run_id, start.seq, |_| ()).is_none() {
            let round = ReviewRound {
                round: 0,
                visit: start.round,
                seq: start.seq,
                reviewers: start.reviewers,
                head_sha: Some(start.head_sha.clone()),
                diff_base: Some(start.diff_base.clone()),
                fix_diff: start.fix_diff,
                failure: None,
                started_at: Utc::now(),
                end: None,
                findings: Vec::new(),
                answered: Vec::new(),
            };
            self.review_rounds
                .lock()
                .expect("rounds")
                .push((run_id, round));
        }
        self.rounds_of_run(run_id)
            .into_iter()
            .find(|round| round.seq == start.seq)
            .ok_or(FakeError)
    }

    async fn fail_review_round(
        &self,
        run_id: RunId,
        failure: &ReviewRoundFailure,
    ) -> Result<ReviewRound, FakeError> {
        if self.with_round(run_id, failure.seq, |_| ()).is_none() {
            let round = ReviewRound {
                round: 0,
                visit: failure.round,
                seq: failure.seq,
                reviewers: failure.reviewers,
                head_sha: None,
                diff_base: None,
                fix_diff: false,
                failure: Some(failure.failure.clone()),
                started_at: Utc::now(),
                end: Some(ReviewRoundEnd {
                    outcome: ReviewRoundOutcome::Failed,
                    ended_at: Utc::now(),
                }),
                findings: Vec::new(),
                answered: Vec::new(),
            };
            self.review_rounds
                .lock()
                .expect("rounds")
                .push((run_id, round));
        }
        self.rounds_of_run(run_id)
            .into_iter()
            .find(|round| round.seq == failure.seq)
            .ok_or(FakeError)
    }

    async fn review_rounds(&self, run_id: RunId) -> Result<Vec<ReviewRound>, FakeError> {
        if self
            .review_rounds_fail
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(FakeError);
        }
        Ok(self.rounds_of_run(run_id))
    }

    async fn runs_review_rounds(
        &self,
        ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<ReviewRound>>, FakeError> {
        let mut rounds = BTreeMap::new();
        for id in ids {
            let of_run = self.rounds_of_run(*id);
            if !of_run.is_empty() {
                rounds.insert(*id, of_run);
            }
        }
        Ok(rounds)
    }

    async fn record_answer(
        &self,
        key: StepKey,
        round_seq: u16,
        findings: &[Finding],
        rulings: &[Ruling],
    ) -> Result<bool, FakeError> {
        if self.with_step(key, |step| step.end.is_some()) != Some(false) {
            return Ok(false);
        }
        let recorded: Vec<ReviewFinding> = (1..)
            .zip(findings)
            .map(|(place, finding)| ReviewFinding {
                seq: key.seq,
                place,
                role: None,
                path: finding.path.clone(),
                lines: finding.lines,
                problem: finding.problem.clone(),
                why: finding.why.clone(),
                blocks_anywhere: finding.blocks_anywhere,
                repeats_dropped: finding.repeats_dropped,
                fix_status: None,
                dispute: None,
                ruling: None,
                ruled_by: None,
                kind: None,
                follow_up_task_id: None,
                follow_up_key: None,
                follow_up_commented: false,
            })
            .collect();
        self.with_round(key.run_id, round_seq, |round| {
            round.findings.retain(|finding| finding.seq != key.seq);
            round.findings.extend(recorded);
            round
                .findings
                .sort_by_key(|finding| (finding.seq, finding.place));
            if !round.answered.contains(&key.seq) {
                round.answered.push(key.seq);
                round.answered.sort_unstable();
            }
        })
        .ok_or(FakeError)?;

        // The Step's rulings replace the ones an earlier attempt made; a
        // dispute another Step ruled on keeps its ruling.
        let mut rounds = self.review_rounds.lock().expect("rounds");
        for finding in rounds
            .iter_mut()
            .filter(|(run, _)| *run == key.run_id)
            .flat_map(|(_, round)| round.findings.iter_mut())
        {
            if finding.ruled_by == Some(key.seq) {
                finding.ruling = None;
                finding.ruled_by = None;
            }
            let ruled = rulings
                .iter()
                .find(|ruling| ruling.seq == finding.seq && ruling.place == finding.place);
            if let Some(ruled) = ruled
                && finding.fix_status == Some(FixStatus::Disputed)
                && finding.ruling.is_none()
            {
                finding.ruling = Some(ruled.ruling);
                finding.ruled_by = Some(key.seq);
            }
        }
        Ok(true)
    }

    async fn record_fix(
        &self,
        run_id: RunId,
        round_seq: u16,
        answers: &[FixAnswer],
    ) -> Result<(), FakeError> {
        self.with_round(run_id, round_seq, |round| {
            for finding in &mut round.findings {
                let answer = answers
                    .iter()
                    .find(|answer| answer.seq == finding.seq && answer.place == finding.place);
                if let Some(answer) = answer
                    && finding.kind == Some(FindingKind::Blocker)
                    && finding.ruling.is_none()
                {
                    finding.fix_status = Some(answer.status);
                    finding.dispute = answer.dispute.clone();
                }
            }
        })
        .ok_or(FakeError)
    }

    async fn reask_action(
        &self,
        key: StepKey,
        _round_seq: u16,
        candidate: Uuid,
    ) -> Result<Uuid, FakeError> {
        let mut asked = self.reasks.lock().expect("reasks");
        if let Some((_, kept)) = asked.iter().find(|(asked, _)| *asked == key) {
            return Ok(*kept);
        }
        asked.push((key, candidate));
        Ok(candidate)
    }

    async fn follow_up_key(
        &self,
        key: StepKey,
        place: u16,
        candidate: Uuid,
    ) -> Result<Option<Uuid>, FakeError> {
        if self.fails("follow_up_key") {
            return Err(FakeError);
        }
        let mut rounds = self.review_rounds.lock().expect("rounds");
        let found = rounds
            .iter_mut()
            .filter(|(run, _)| *run == key.run_id)
            .flat_map(|(_, round)| round.findings.iter_mut())
            .find(|finding| finding.seq == key.seq && finding.place == place)
            .filter(|finding| {
                finding.kind == Some(factory_records::domain::review_rounds::FindingKind::FollowUp)
            });
        Ok(found.map(|finding| *finding.follow_up_key.get_or_insert(candidate)))
    }

    async fn classify_findings(
        &self,
        run_id: RunId,
        round_seq: u16,
        classes: &[FindingClass],
    ) -> Result<(), FakeError> {
        self.with_round(run_id, round_seq, |round| {
            for class in classes {
                let found = round
                    .findings
                    .iter_mut()
                    .find(|finding| finding.seq == class.seq && finding.place == class.place);
                if let Some(finding) = found {
                    finding.kind.get_or_insert(class.kind);
                }
            }
        });
        Ok(())
    }

    async fn link_follow_up(
        &self,
        key: StepKey,
        place: u16,
        task_id: Uuid,
    ) -> Result<(), FakeError> {
        if self.fails("link_follow_up") {
            return Err(FakeError);
        }
        let mut rounds = self.review_rounds.lock().expect("rounds");
        let found = rounds
            .iter_mut()
            .filter(|(run, _)| *run == key.run_id)
            .flat_map(|(_, round)| round.findings.iter_mut())
            .find(|finding| finding.seq == key.seq && finding.place == place);
        if let Some(finding) = found
            && finding.kind == Some(factory_records::domain::review_rounds::FindingKind::FollowUp)
        {
            finding.follow_up_task_id.get_or_insert(task_id);
        }
        Ok(())
    }

    async fn mark_follow_up_commented(
        &self,
        run_id: RunId,
        round_seq: u16,
        task_id: Uuid,
    ) -> Result<(), FakeError> {
        if self.fails("mark_follow_up_commented") {
            return Err(FakeError);
        }
        self.with_round(run_id, round_seq, |round| {
            for finding in &mut round.findings {
                if finding.follow_up_task_id == Some(task_id) {
                    finding.follow_up_commented = true;
                }
            }
        });
        Ok(())
    }

    async fn end_review_round(
        &self,
        run_id: RunId,
        seq: u16,
        outcome: ReviewRoundOutcome,
    ) -> Result<Option<ReviewRound>, FakeError> {
        self.with_round(run_id, seq, |round| {
            round.end.get_or_insert(ReviewRoundEnd {
                outcome,
                ended_at: Utc::now(),
            });
        });
        Ok(self
            .rounds_of_run(run_id)
            .into_iter()
            .find(|round| round.seq == seq))
    }
}
