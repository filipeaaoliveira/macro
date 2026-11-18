//! Experiment arms, Scorers' judgments and Benchmark suites in memory, with
//! the same first-write-wins rules as the Postgres repository.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use factory_records::domain::evaluation::{
    AgentChoices, EvaluationRecords, EvaluationRepository, Judgment, NewSuite, RunArm, ScoringStep,
    ScoringSubject, SubjectTrial, TrialKey, TrialResult,
};
use factory_records::domain::insights::InsightsScope;
use factory_records::domain::models::RunId;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::ports::{JudgeQuestion, JudgeVerdict, RunJudge};

use super::{FakeError, FakeRepo};

/// A suite kept: its Benchmark and configuration.
#[derive(Debug, Clone)]
pub(crate) struct FakeSuite {
    pub(crate) benchmark: String,
    pub(crate) agents: AgentChoices,
}

/// What [`FakeRepo`] keeps of Experiments, Scorers and Benchmarks.
#[derive(Debug, Clone, Default)]
pub(crate) struct FakeEvaluation {
    /// Each Run's arms.
    pub(crate) arms: BTreeMap<RunId, Vec<RunArm>>,
    /// The live Scorers' judgments, by Run.
    pub(crate) run_scores: BTreeMap<RunId, Vec<Judgment>>,
    /// The suites, by id.
    pub(crate) suites: BTreeMap<Uuid, FakeSuite>,
    /// Each trial's Run.
    pub(crate) trials: BTreeMap<(Uuid, String), RunId>,
    /// Each trial's result.
    pub(crate) trial_results: BTreeMap<(Uuid, String), TrialResult>,
    /// Each trial's judgments.
    pub(crate) trial_scores: BTreeMap<(Uuid, String), Vec<Judgment>>,
}

fn trial_id(trial: &TrialKey) -> (Uuid, String) {
    (trial.suite_id, trial.task.clone())
}

impl EvaluationRepository for FakeRepo {
    type Err = FakeError;

    async fn record_arms(&self, run_id: RunId, arms: &[RunArm]) -> Result<(), FakeError> {
        if self.fails("record_arms") {
            return Err(FakeError);
        }
        if arms.is_empty() {
            return Ok(());
        }
        let mut kept = self.evaluation.lock().expect("evaluation");
        kept.arms.entry(run_id).or_insert_with(|| arms.to_vec());
        Ok(())
    }

    async fn run_agents(&self, run_id: RunId) -> Result<AgentChoices, FakeError> {
        let kept = self.evaluation.lock().expect("evaluation");
        let mut agents = AgentChoices::new();
        for arm in kept.arms.get(&run_id).into_iter().flatten() {
            agents.extend(arm.agents.clone());
        }
        let suite = kept
            .trials
            .iter()
            .find(|(_, run)| **run == run_id)
            .and_then(|((suite_id, _), _)| kept.suites.get(suite_id));
        if let Some(suite) = suite {
            agents.extend(suite.agents.clone());
        }
        Ok(agents)
    }

    async fn scoring_subject(&self, run_id: RunId) -> Result<Option<ScoringSubject>, FakeError> {
        let Some(run) = self.runs.lock().expect("runs").get(&run_id).cloned() else {
            return Ok(None);
        };
        let flow = self.flows.lock().expect("flows").get(&run_id).cloned();
        let sessions = self.sessions.lock().expect("sessions").clone();
        let steps = self
            .steps
            .lock()
            .expect("steps")
            .iter()
            .filter(|(id, _)| *id == run_id)
            .map(|(_, step)| ScoringStep {
                entry: step.entry.clone(),
                outcome: step.end.as_ref().map(|end| end.outcome.clone()),
                output: step.end.as_ref().map(|end| end.output.clone()),
                session_id: sessions
                    .iter()
                    .find(|session| session.key.run_id == run_id && session.key.seq == step.seq)
                    .map(|session| session.session.session_id),
            })
            .collect();
        let kept = self.evaluation.lock().expect("evaluation");
        let trial =
            kept.trials
                .iter()
                .find(|(_, run)| **run == run_id)
                .map(|((suite_id, task), _)| {
                    let key = TrialKey {
                        suite_id: *suite_id,
                        task: task.clone(),
                    };
                    SubjectTrial {
                        benchmark: kept.suites[suite_id].benchmark.clone(),
                        scored: kept.trial_results.contains_key(&trial_id(&key)),
                        key,
                    }
                });
        let judged = match &trial {
            Some(trial) => kept.trial_scores.get(&trial_id(&trial.key)),
            None => kept.run_scores.get(&run_id),
        }
        .into_iter()
        .flatten()
        .map(|judgment| judgment.scorer.clone())
        .collect();
        Ok(Some(ScoringSubject {
            run_id,
            user_id: run.user_id,
            flow,
            request: run.request,
            outcome: run.end.map(|end| end.outcome),
            steps,
            trial,
            judged,
        }))
    }

    async fn record_run_score(&self, run_id: RunId, judgment: &Judgment) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        let scores = kept.run_scores.entry(run_id).or_default();
        if !scores.iter().any(|kept| kept.scorer == judgment.scorer) {
            scores.push(judgment.clone());
        }
        Ok(())
    }

    async fn record_trial_score(
        &self,
        trial: &TrialKey,
        judgment: &Judgment,
    ) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        let scores = kept.trial_scores.entry(trial_id(trial)).or_default();
        if !scores.iter().any(|kept| kept.scorer == judgment.scorer) {
            scores.push(judgment.clone());
        }
        Ok(())
    }

    async fn record_trial_result(
        &self,
        trial: &TrialKey,
        result: &TrialResult,
    ) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        kept.trial_results.entry(trial_id(trial)).or_insert(*result);
        Ok(())
    }

    async fn insert_suite(&self, suite: &NewSuite<'_>) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        kept.suites.entry(suite.id).or_insert_with(|| FakeSuite {
            benchmark: suite.benchmark.to_string(),
            agents: suite.agents.clone(),
        });
        Ok(())
    }

    async fn insert_trial(&self, trial: &TrialKey, run_id: RunId) -> Result<(), FakeError> {
        if self.fails("insert_trial") {
            return Err(FakeError);
        }
        let mut kept = self.evaluation.lock().expect("evaluation");
        kept.trials.entry(trial_id(trial)).or_insert(run_id);
        Ok(())
    }

    async fn read_evaluation(
        &self,
        _scope: &InsightsScope,
    ) -> Result<EvaluationRecords, FakeError> {
        Ok(EvaluationRecords::default())
    }
}

/// A Scorer's model that answers each call with the next verdict it was
/// given, or 0.8 once none is left, and keeps what it was asked.
#[derive(Default)]
pub(crate) struct FakeJudge {
    verdicts: Mutex<VecDeque<Result<JudgeVerdict, FakeError>>>,
    asked: Mutex<Vec<(MacroUserIdStr<'static>, JudgeQuestion)>>,
}

impl FakeJudge {
    /// Answer the next call with `score`.
    pub(crate) fn then(&self, score: f64) {
        self.verdicts
            .lock()
            .expect("verdicts")
            .push_back(Ok(JudgeVerdict {
                score,
                reason: "It does what the request asks.".to_string(),
            }));
    }

    /// Fail the next call.
    pub(crate) fn then_fail(&self) {
        self.verdicts
            .lock()
            .expect("verdicts")
            .push_back(Err(FakeError));
    }

    /// What it was asked, and for whom.
    pub(crate) fn asked(&self) -> Vec<(MacroUserIdStr<'static>, JudgeQuestion)> {
        self.asked.lock().expect("asked").clone()
    }
}

impl RunJudge for FakeJudge {
    type Err = FakeError;

    fn model(&self) -> &str {
        "fake/judge"
    }

    async fn judge(
        &self,
        owner: &MacroUserIdStr<'static>,
        question: &JudgeQuestion,
    ) -> Result<JudgeVerdict, FakeError> {
        self.asked
            .lock()
            .expect("asked")
            .push((owner.clone(), question.clone()));
        self.verdicts
            .lock()
            .expect("verdicts")
            .pop_front()
            .unwrap_or(Ok(JudgeVerdict {
                score: 0.8,
                reason: "It does what the request asks.".to_string(),
            }))
    }
}

/// A shared judge is a judge, so a test keeps a handle on what it was asked.
impl RunJudge for std::sync::Arc<FakeJudge> {
    type Err = FakeError;

    fn model(&self) -> &str {
        "fake/judge"
    }

    async fn judge(
        &self,
        owner: &MacroUserIdStr<'static>,
        question: &JudgeQuestion,
    ) -> Result<JudgeVerdict, FakeError> {
        self.as_ref().judge(owner, question).await
    }
}
