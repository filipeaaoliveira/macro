//! A Run's review rounds: a panel of reviewers reviews the Run's change at
//! once, each in a Step of its own, and hands back the Blockers it found,
//! with where they are and why. The round records the commit it reviews and
//! where the diff under review starts, and once it ends, which findings were
//! Blockers in that diff and which became follow-up tasks.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::models::{Role, RunId, StepKey};

/// The most findings one reviewer hands back in a round.
pub const MAX_FINDINGS: usize = 50;

/// The most bytes a finding's path may hold (the
/// `factory_review_finding.path` check).
pub const MAX_FINDING_PATH_BYTES: usize = 1_024;

/// The most bytes a finding's problem, or why it matters, may hold (the
/// `factory_review_finding` checks).
pub const MAX_FINDING_TEXT_BYTES: usize = 4_096;

/// The most bytes a fix's reason to dispute a Blocker keeps (the
/// `factory_review_finding_dispute` check); a longer one is cut.
pub const MAX_DISPUTE_BYTES: usize = 2_048;

/// The lines of a file a finding is about, from 1, both included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Lines {
    /// The first line.
    pub start: u32,
    /// The last line; the first, for one line.
    pub end: u32,
}

/// What a reviewer found: where it is, what is wrong, and why it matters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The file, from the repository's root.
    pub path: String,
    /// The lines it is about; none for the whole file.
    pub lines: Option<Lines>,
    /// What is wrong.
    pub problem: String,
    /// Why it must change.
    pub why: String,
    /// Whether it blocks wherever it is: a Blocker of the round before
    /// found again, or a reviewer's answer that never read.
    pub blocks_anywhere: bool,
    /// Whether its reviewer raises again a Blocker it dropped in this
    /// visit of its entry: it is then dismissed, and blocks nothing.
    pub repeats_dropped: bool,
}

/// What a finding turned out to be once its round ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// In the diff under review: the fix must change it before the pull
    /// request goes on.
    Blocker,
    /// Outside the diff under review: a follow-up task of its own, which
    /// does not hold the pull request.
    FollowUp,
    /// A Blocker its reviewer dropped earlier in the visit, raised again:
    /// kept, but it blocks nothing and becomes no task.
    Dismissed,
}

impl FindingKind {
    /// The word the kind is kept as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocker => "blocker",
            Self::FollowUp => "follow_up",
            Self::Dismissed => "dismissed",
        }
    }
}

/// What the fix said of a Blocker it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FixStatus {
    /// It fixed it.
    Fixed,
    /// It disputes it, with why: the next round's reviewer rules on it.
    Disputed,
}

impl FixStatus {
    /// The status or ruling kept as `word`, if it is one.
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "fixed" => Some(Self::Fixed),
            "disputed" => Some(Self::Disputed),
            _ => None,
        }
    }

    /// The word the status is kept as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::Disputed => "disputed",
        }
    }
}

/// How the next round's reviewer ruled on a disputed Blocker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DisputeRuling {
    /// The Blocker stands, and blocks again: the reviewer listed it again,
    /// or did not drop it.
    Upheld,
    /// The reviewer dropped it: it is dismissed and blocks nothing.
    Dropped,
}

impl DisputeRuling {
    /// The status or ruling kept as `word`, if it is one.
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "upheld" => Some(Self::Upheld),
            "dropped" => Some(Self::Dropped),
            _ => None,
        }
    }

    /// The word the ruling is kept as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Upheld => "upheld",
            Self::Dropped => "dropped",
        }
    }
}

/// What the fix said of the Blocker at `seq` and `place`, for
/// [`ReviewRoundRepository::record_fix`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixAnswer {
    /// The reviewer's Step that found the Blocker.
    pub seq: u16,
    /// The Blocker's place among what the reviewer handed back.
    pub place: u16,
    /// Fixed or disputed.
    pub status: FixStatus,
    /// Why it is disputed, at most [`MAX_DISPUTE_BYTES`]; none when fixed.
    pub dispute: Option<String>,
}

/// A reviewer's ruling on the disputed Blocker of the round before at
/// `seq` and `place`, for [`ReviewRoundRepository::record_findings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ruling {
    /// The reviewer's Step that found the disputed Blocker.
    pub seq: u16,
    /// Its place.
    pub place: u16,
    /// Upheld or dropped.
    pub ruling: DisputeRuling,
}

/// What became of a Run's Blockers: fixed as the fixer says, or disputed
/// and then upheld or dropped, and the findings dismissed as repeats of a
/// dropped one. A dispute no round ruled on yet counts as upheld.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlockerResolutions {
    /// Blockers the fixer said it fixed: its claim, which the next round
    /// checks.
    pub fixed: u32,
    /// Blockers the fix disputed that a reviewer upheld, or did not drop.
    pub disputed_upheld: u32,
    /// Blockers the fix disputed that a reviewer dropped.
    pub disputed_dropped: u32,
    /// Findings dismissed: a reviewer raised again, in a later round of the
    /// visit, the same problem at the same lines as a Blocker it dropped.
    pub dismissed: u32,
}

impl BlockerResolutions {
    /// Count one Blocker the fix answered with `status`, and ruled on
    /// with `ruling`.
    pub fn count(&mut self, status: Option<FixStatus>, ruling: Option<DisputeRuling>) {
        match (status, ruling) {
            (Some(FixStatus::Fixed), _) => self.fixed += 1,
            (Some(FixStatus::Disputed), Some(DisputeRuling::Dropped)) => {
                self.disputed_dropped += 1;
            }
            (Some(FixStatus::Disputed), _) => self.disputed_upheld += 1,
            (None, _) => {}
        }
    }

    /// What became of the Blockers of a Run's `rounds`, and its findings
    /// dismissed.
    pub fn of_rounds(rounds: &[ReviewRound]) -> Self {
        let mut counts = Self::default();
        for blocker in rounds.iter().flat_map(ReviewRound::blockers) {
            counts.count(blocker.fix_status, blocker.ruling);
        }
        counts.dismissed = u32::try_from(
            rounds
                .iter()
                .flat_map(|round| &round.findings)
                .filter(|finding| finding.kind == Some(FindingKind::Dismissed))
                .count(),
        )
        .unwrap_or(u32::MAX);
        counts
    }
}

/// A finding as its round keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFinding {
    /// The reviewer's Step: its place among the Run's Steps.
    pub seq: u16,
    /// Its place among what the reviewer handed back, from 1.
    pub place: u16,
    /// The Role of the reviewer that found it.
    pub role: Option<Role>,
    /// The file, from the repository's root.
    pub path: String,
    /// The lines it is about; none for the whole file.
    pub lines: Option<Lines>,
    /// What is wrong.
    pub problem: String,
    /// Why it must change.
    pub why: String,
    /// Whether it blocks wherever it is: a Blocker of the round before
    /// found again, or a reviewer's answer that never read.
    pub blocks_anywhere: bool,
    /// Whether its reviewer raised again a Blocker it dropped in this
    /// visit: it is then dismissed.
    #[serde(skip)]
    pub repeats_dropped: bool,
    /// What the fix said of it, when it was a Blocker the fix was given.
    pub fix_status: Option<FixStatus>,
    /// Why the fix disputes it, as the fixer wrote it.
    pub dispute: Option<String>,
    /// How the next round's reviewer ruled on the dispute; none until a
    /// reviewer answered.
    pub ruling: Option<DisputeRuling>,
    /// The reviewer's Step that ruled on the dispute.
    #[serde(skip)]
    pub ruled_by: Option<u16>,
    /// What it turned out to be; none while its round reviews.
    pub kind: Option<FindingKind>,
    /// The follow-up task it became, when it is a follow-up and the task
    /// was made: its task document's id.
    pub follow_up_task_id: Option<Uuid>,
    /// The id its follow-up task is made with, kept before it is asked
    /// for, so a retry finds the task rather than making a second one.
    #[serde(skip)]
    pub follow_up_key: Option<Uuid>,
    /// Whether the owner commented on its follow-up task, one an earlier
    /// round made, that its round reported the problem again.
    #[serde(skip)]
    pub follow_up_commented: bool,
}

/// How a review round ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewRoundOutcome {
    /// No Blocker in the diff under review.
    Pass,
    /// One or more Blockers, for the fix.
    Changes,
    /// A reviewer's work failed.
    Failed,
}

impl ReviewRoundOutcome {
    /// The word the outcome is kept as, which is also the outcome the Flow
    /// routes.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Changes => "changes",
            Self::Failed => "failed",
        }
    }
}

/// A review round that has ended.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRoundEnd {
    /// How it ended.
    pub outcome: ReviewRoundOutcome,
    /// When.
    pub ended_at: DateTime<Utc>,
}

/// One round of a Run's review panel.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewRound {
    /// Which round of the Run it is, from 1, counting every round the Run
    /// recorded: a reset of its entry does not count again from 1.
    pub round: u16,
    /// Which round of its entry it is since the entry was last reset, from
    /// 1, as the entry's round limit counts them: a round after a reset
    /// (as when a person sends the change back) is visit 1 again, and
    /// reviews the whole change.
    pub visit: u16,
    /// The place of its first Step among the Run's Steps; its reviewers'
    /// Steps take this place and the ones after it.
    pub seq: u16,
    /// How many reviewers it has, each in a Step of its own.
    pub reviewers: u16,
    /// The commit of the Run's branch it reviews; none for a round that
    /// could not open.
    pub head_sha: Option<String>,
    /// Where the diff under review starts: the Run's base branch, or the
    /// commit the round before reviewed; none for a round that could not
    /// open.
    pub diff_base: Option<String>,
    /// Whether it reviews the fix diff, the changes made since the round
    /// before, rather than the Run's whole change.
    pub fix_diff: bool,
    /// When it started.
    pub started_at: DateTime<Utc>,
    /// Why it could not open, as when the Run's branch is gone: it then
    /// ran no reviewer and ended failed.
    pub failure: Option<String>,
    /// How and when it ended; none while it reviews.
    pub end: Option<ReviewRoundEnd>,
    /// What its reviewers found, by Step and place.
    pub findings: Vec<ReviewFinding>,
    /// The reviewers' Steps whose answers were read, so their findings are
    /// recorded, in order.
    #[serde(skip)]
    pub answered: Vec<u16>,
}

impl ReviewRound {
    /// The places of its reviewers' Steps.
    pub fn step_seqs(&self) -> std::ops::Range<u16> {
        self.seq..self.seq.saturating_add(self.reviewers)
    }

    /// Its findings that turned out Blockers.
    pub fn blockers(&self) -> impl Iterator<Item = &ReviewFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.kind == Some(FindingKind::Blocker))
    }
}

/// Number the Run's `rounds`, given in the order of their Steps, from 1: each
/// one's [`ReviewRound::round`] is its place among them, whatever its
/// visit.
pub fn number_rounds(rounds: &mut [ReviewRound]) {
    for (number, round) in (1..).zip(rounds.iter_mut()) {
        round.round = number;
    }
}

/// A review round to record, before its reviewers start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRoundStart {
    /// The place of its first Step.
    pub seq: u16,
    /// Which round of its entry since its last reset, from 1: its
    /// [`ReviewRound::visit`].
    pub round: u16,
    /// How many reviewers it has.
    pub reviewers: u16,
    /// The commit it reviews.
    pub head_sha: String,
    /// Where the diff under review starts.
    pub diff_base: String,
    /// Whether it reviews the fix diff.
    pub fix_diff: bool,
}

/// A review round that could not open, to record as ended failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewRoundFailure {
    /// The place of its first Step.
    pub seq: u16,
    /// Which round of its entry since its last reset, from 1: its
    /// [`ReviewRound::visit`].
    pub round: u16,
    /// How many reviewers it would have had.
    pub reviewers: u16,
    /// Why it could not open, for a person.
    pub failure: String,
}

/// The most bytes [`ReviewRoundFailure::failure`] keeps.
pub const MAX_FAILURE_BYTES: usize = 4_096;

/// What a finding turned out to be, for [`ReviewRoundRepository::classify_findings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindingClass {
    /// The reviewer's Step.
    pub seq: u16,
    /// The finding's place.
    pub place: u16,
    /// What it is.
    pub kind: FindingKind,
}

/// Where a Run's review rounds are kept. A round's workflow repeats its
/// writes when Temporal retries them, so each is safe to repeat: the first
/// record, classification, task and end stay.
pub trait ReviewRoundRepository: Send + Sync + 'static {
    /// The error type returned by repository operations.
    type Err: std::error::Error + Send + Sync + 'static;

    /// Record that the Run `run_id` starts the review round `start`, and
    /// return the round as recorded: recording it again keeps the first
    /// record.
    fn start_review_round(
        &self,
        run_id: RunId,
        start: &ReviewRoundStart,
    ) -> impl Future<Output = Result<ReviewRound, Self::Err>> + Send;

    /// Record that the Run `run_id`'s review round `failure` could not open:
    /// it ends failed, with why. Recording it again keeps the first record.
    fn fail_review_round(
        &self,
        run_id: RunId,
        failure: &ReviewRoundFailure,
    ) -> impl Future<Output = Result<ReviewRound, Self::Err>> + Send;

    /// The Run's review rounds, in order, each with its findings.
    fn review_rounds(
        &self,
        run_id: RunId,
    ) -> impl Future<Output = Result<Vec<ReviewRound>, Self::Err>> + Send;

    /// The review rounds of the Runs `ids`, each Run's in order. A Run with
    /// no round has no entry.
    fn runs_review_rounds(
        &self,
        ids: &[RunId],
    ) -> impl Future<Output = Result<BTreeMap<RunId, Vec<ReviewRound>>, Self::Err>> + Send;

    /// Record what the reviewer of the Step `key`, in the round that starts
    /// at `round_seq`, found, and that its answer was read, replacing what
    /// an earlier attempt of the Step recorded, while the Step has not
    /// ended. Whether it was recorded: false once the Step ended.
    fn record_findings(
        &self,
        key: StepKey,
        round_seq: u16,
        findings: &[Finding],
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send {
        self.record_answer(key, round_seq, findings, &[])
    }

    /// Record a reviewer's answer as [`Self::record_findings`] does, with
    /// its `rulings` on the disputed Blockers of the round before: they
    /// replace the rulings an earlier attempt of the Step made, and a
    /// dispute another Step ruled on keeps its ruling.
    fn record_answer(
        &self,
        key: StepKey,
        round_seq: u16,
        findings: &[Finding],
        rulings: &[Ruling],
    ) -> impl Future<Output = Result<bool, Self::Err>> + Send;

    /// Record what the fix said of the Blockers of the Run's round
    /// `round_seq`, replacing what it said before, on the Blockers no
    /// reviewer has ruled on yet: a ruled dispute keeps what the fix said.
    fn record_fix(
        &self,
        run_id: RunId,
        round_seq: u16,
        answers: &[FixAnswer],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// The prompt in which the reviewer of the Step `key`, in the round
    /// that starts at `round_seq`, is asked once more for an answer that
    /// reads: `candidate`, unless an earlier attempt kept one.
    fn reask_action(
        &self,
        key: StepKey,
        round_seq: u16,
        candidate: Uuid,
    ) -> impl Future<Output = Result<Uuid, Self::Err>> + Send;

    /// The id the follow-up at `place` of the Step `key` is made with:
    /// `candidate`, unless an earlier attempt kept one. None when the
    /// finding is not a follow-up.
    fn follow_up_key(
        &self,
        key: StepKey,
        place: u16,
        candidate: Uuid,
    ) -> impl Future<Output = Result<Option<Uuid>, Self::Err>> + Send;

    /// Record what the findings of the Run's round `round_seq` turned out
    /// to be. A finding already classified keeps its first kind.
    fn classify_findings(
        &self,
        run_id: RunId,
        round_seq: u16,
        classes: &[FindingClass],
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the follow-up at `place` of the Step `key` became the
    /// task `task_id`. A follow-up that already has its task keeps it.
    fn link_follow_up(
        &self,
        key: StepKey,
        place: u16,
        task_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record that the owner commented on the follow-up task `task_id`
    /// that the Run's round `round_seq` reported its problem again: on each
    /// of the round's findings linked to it.
    fn mark_follow_up_commented(
        &self,
        run_id: RunId,
        round_seq: u16,
        task_id: Uuid,
    ) -> impl Future<Output = Result<(), Self::Err>> + Send;

    /// Record how the Run's round `seq` ended. A round that already ended
    /// keeps its first end. The round as recorded.
    fn end_review_round(
        &self,
        run_id: RunId,
        seq: u16,
        outcome: ReviewRoundOutcome,
    ) -> impl Future<Output = Result<Option<ReviewRound>, Self::Err>> + Send;
}
