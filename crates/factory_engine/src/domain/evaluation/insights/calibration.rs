//! Calibration: a Run's owner labels a live Scorer's judgment of the Run
//! with whether it should have passed, and a Scorer counts as calibrated
//! once its judge agrees with the person's labels often enough. Until then
//! its numbers are shown, and nothing should act on them.

#[cfg(test)]
mod test;

use chrono::{DateTime, Utc};
use factory_records::domain::evaluation::{
    CalibrationTally, EvaluationRepository, PASS_SCORE, ScorerLabel,
};
use factory_records::domain::models::RunId;
use factory_records::domain::ports::RunRepository;
use macro_user_id::user_id::MacroUserIdStr;
use serde::Serialize;

use super::{EvaluationServiceImpl, internal};

/// The judgments a person labels before a Scorer can be calibrated.
pub const CALIBRATION_LABELS: u32 = 20;

/// The share of those labels its judge must agree with.
pub const CALIBRATION_AGREEMENT: f64 = 0.8;

/// How far a Scorer's judge agrees with a person's labels of its judgments
/// of their Runs, over all of their Runs.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScorerCalibration {
    /// How many of its judgments the person labelled.
    pub labelled: u32,
    /// How many of those it passed exactly when the person did.
    pub agreed: u32,
    /// `agreed` over `labelled`; none when none is labelled.
    pub agreement: Option<f64>,
    /// The labels it needs to be calibrated.
    pub needed: u32,
    /// The agreement it needs.
    pub threshold: f64,
    /// Whether it has both: until then, nothing should act on its numbers.
    pub calibrated: bool,
}

impl ScorerCalibration {
    /// The calibration of a Scorer `tally` counts; none labelled without
    /// one.
    pub fn of(tally: Option<&CalibrationTally>) -> Self {
        let (labelled, agreed) = tally.map_or((0, 0), |tally| (tally.labelled, tally.agreed));
        let agreement = (labelled > 0).then(|| f64::from(agreed) / f64::from(labelled));
        let calibrated = labelled >= CALIBRATION_LABELS
            && agreement.is_some_and(|share| share >= CALIBRATION_AGREEMENT);
        Self {
            labelled,
            agreed,
            agreement,
            needed: CALIBRATION_LABELS,
            threshold: CALIBRATION_AGREEMENT,
            calibrated,
        }
    }
}

/// A live Scorer's judgment of one Run, as the Run view shows it.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunScore {
    /// The Scorer.
    pub scorer: String,
    /// From 0 to 1.
    pub score: f64,
    /// Whether the score passes: at least half.
    pub passed: bool,
    /// Why, as the judge said.
    pub reason: String,
    /// The model that judged.
    pub model: String,
    /// When it judged.
    pub judged_at: DateTime<Utc>,
    /// The Run's owner's label of it, if they gave one.
    pub label: Option<ScorerLabel>,
    /// How far the Scorer agrees with the owner's labels.
    pub calibration: ScorerCalibration,
}

/// The live Scorers' judgments of one Run.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunScores {
    /// One for each Scorer that judged it, by Scorer.
    pub scores: Vec<RunScore>,
}

/// Why reading or labelling a Run's judgments was refused.
#[derive(Debug, thiserror::Error)]
pub enum ScorerLabelError {
    /// The caller has no Run of that id.
    #[error("run not found")]
    NotFound,
    /// The Scorer did not judge the Run, so there is nothing to label.
    #[error("scorer {0} did not judge this run")]
    NotJudged(String),
    /// The caller is not the person in their own signed-in session, or
    /// signs in as an agent's account.
    #[error("only the person, signed in to Macro, can label a run")]
    NotAPerson,
    /// Anything else.
    #[error("reading or labelling a run's judgments failed: {0:?}")]
    Internal(rootcause::Report),
}

fn label_internal(error: impl std::error::Error + Send + Sync + 'static) -> ScorerLabelError {
    ScorerLabelError::Internal(internal(error))
}

/// Reading and labelling a Run's live judgments.
pub trait ScorerLabelService: Send + Sync + 'static {
    /// The live Scorers' judgments of `actor`'s Run `run_id`, with their
    /// labels and each Scorer's calibration.
    fn run_scores(
        &self,
        actor: &MacroUserIdStr<'static>,
        run_id: RunId,
    ) -> impl Future<Output = Result<RunScores, ScorerLabelError>> + Send;

    /// Label, as `actor`, `scorer`'s judgment of their Run `run_id`:
    /// whether it should have passed. A new label replaces the last.
    fn label_score(
        &self,
        actor: &MacroUserIdStr<'static>,
        run_id: RunId,
        scorer: &str,
        passed: bool,
    ) -> impl Future<Output = Result<ScorerLabel, ScorerLabelError>> + Send;
}

impl<R, F> EvaluationServiceImpl<R, F>
where
    R: RunRepository,
{
    /// Fails unless `run_id` is `actor`'s Run.
    async fn own_run(
        &self,
        actor: &MacroUserIdStr<'static>,
        run_id: RunId,
    ) -> Result<(), ScorerLabelError> {
        let run = self.repo.get_run(run_id).await.map_err(label_internal)?;
        if run.is_some_and(|run| run.user_id == *actor) {
            return Ok(());
        }
        Err(ScorerLabelError::NotFound)
    }
}

impl<R, F> ScorerLabelService for EvaluationServiceImpl<R, F>
where
    R: EvaluationRepository + RunRepository,
    F: Send + Sync + 'static,
{
    #[tracing::instrument(skip(self, actor))]
    async fn run_scores(
        &self,
        actor: &MacroUserIdStr<'static>,
        run_id: RunId,
    ) -> Result<RunScores, ScorerLabelError> {
        self.own_run(actor, run_id).await?;
        let judgments = self
            .repo
            .run_judgments(run_id)
            .await
            .map_err(label_internal)?;
        let tallies = self
            .repo
            .calibrations(actor)
            .await
            .map_err(label_internal)?;
        let scores = judgments
            .into_iter()
            .map(|judgment| {
                let tally = tallies.iter().find(|tally| tally.scorer == judgment.scorer);
                RunScore {
                    passed: judgment.score >= PASS_SCORE,
                    calibration: ScorerCalibration::of(tally),
                    scorer: judgment.scorer,
                    score: judgment.score,
                    reason: judgment.reason,
                    model: judgment.model,
                    judged_at: judgment.judged_at,
                    label: judgment.label,
                }
            })
            .collect();
        Ok(RunScores { scores })
    }

    #[tracing::instrument(skip(self, actor))]
    async fn label_score(
        &self,
        actor: &MacroUserIdStr<'static>,
        run_id: RunId,
        scorer: &str,
        passed: bool,
    ) -> Result<ScorerLabel, ScorerLabelError> {
        // A label says what a person judged: an agent's account signs in as
        // a person does, yet never is one.
        if self
            .repo
            .is_agent_account(actor)
            .await
            .map_err(label_internal)?
        {
            return Err(ScorerLabelError::NotAPerson);
        }
        self.own_run(actor, run_id).await?;
        self.repo
            .label_judgment(run_id, scorer, passed)
            .await
            .map_err(label_internal)?
            .ok_or_else(|| ScorerLabelError::NotJudged(scorer.to_string()))
    }
}
