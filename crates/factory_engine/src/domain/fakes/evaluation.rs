//! Experiment arms, Scorers' judgments and Benchmark suites in memory, with
//! the same first-write-wins rules as the Postgres repository.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use factory_records::domain::evaluation::{
    AgentChoices, EvaluationRecords, EvaluationRepository, Judgment, MAX_OPEN_TRIALS, NewSuite,
    RunAgents, RunArm, ScoringStep, ScoringSubject, SubjectTrial, SuiteInsert, TrialKey,
    TrialResult,
};
use factory_records::domain::insights::InsightsScope;
use factory_records::domain::models::RunId;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::ports::{JudgeQuestion, JudgeVerdict, RunJudge};

use super::{FakeError, FakeRepo};

/// A suite kept: whose, its Benchmark and configuration.
#[derive(Debug, Clone)]
pub(crate) struct FakeSuite {
    pub(crate) owner: String,
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
    /// The commit each trial's write starts from.
    pub(crate) base_commits: BTreeMap<(Uuid, String), String>,
    /// The trials whose branch was deleted.
    pub(crate) branches_deleted: Vec<(Uuid, String)>,
}

impl FakeEvaluation {
    /// The trial `run_id` is, with its id.
    fn trial_of(&self, run_id: RunId) -> Option<SubjectTrial> {
        self.trials
            .iter()
            .find(|(_, run)| **run == run_id)
            .map(|((suite_id, task), _)| {
                let key = TrialKey {
                    suite_id: *suite_id,
                    task: task.clone(),
                };
                SubjectTrial {
                    benchmark: self.suites[suite_id].benchmark.clone(),
                    scored: self.trial_results.contains_key(&trial_id(&key)),
                    branch_deleted: self.branches_deleted.contains(&trial_id(&key)),
                    key,
                }
            })
    }
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

    async fn run_agents(&self, run_id: RunId) -> Result<RunAgents, FakeError> {
        let kept = self.evaluation.lock().expect("evaluation");
        let mut agents = RunAgents::default();
        for arm in kept.arms.get(&run_id).into_iter().flatten() {
            agents.choices.extend(arm.agents.clone());
        }
        let suite = kept
            .trials
            .iter()
            .find(|(_, run)| **run == run_id)
            .and_then(|((suite_id, _), _)| kept.suites.get(suite_id));
        if let Some(suite) = suite {
            agents.choices.extend(suite.agents.clone());
            agents.trial = true;
        }
        Ok(agents)
    }

    async fn trial_of(&self, run_id: RunId) -> Result<Option<SubjectTrial>, FakeError> {
        Ok(self.evaluation.lock().expect("evaluation").trial_of(run_id))
    }

    async fn trial_base_commit(&self, run_id: RunId) -> Result<Option<String>, FakeError> {
        let kept = self.evaluation.lock().expect("evaluation");
        Ok(kept
            .trial_of(run_id)
            .and_then(|trial| kept.base_commits.get(&trial_id(&trial.key)).cloned()))
    }

    async fn record_branch_deleted(&self, trial: &TrialKey) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        if !kept.branches_deleted.contains(&trial_id(trial)) {
            kept.branches_deleted.push(trial_id(trial));
        }
        Ok(())
    }

    async fn delete_suite(&self, suite_id: Uuid) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        kept.suites.remove(&suite_id);
        kept.trials.retain(|(suite, _), _| *suite != suite_id);
        Ok(())
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
        let trial = kept.trial_of(run_id);
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
            repo_url: run.repository.map(|repository| repository.url),
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

    async fn insert_suite(&self, suite: &NewSuite<'_>) -> Result<SuiteInsert, FakeError> {
        let runs = self.runs.lock().expect("runs").clone();
        let mut kept = self.evaluation.lock().expect("evaluation");
        let owner = suite.user_id.as_ref().to_string();
        // A suite runs while one of its trials' Runs goes, or while it has
        // no trial yet.
        let going = |suite_id: &Uuid| {
            let trials: Vec<_> = kept
                .trials
                .iter()
                .filter(|((suite, _), _)| suite == suite_id)
                .map(|(_, run)| *run)
                .collect();
            trials.is_empty()
                || trials
                    .iter()
                    .any(|run| runs.get(run).is_some_and(|run| run.end.is_none()))
        };
        let running = kept.suites.iter().any(|(id, kept_suite)| {
            kept_suite.owner == owner && kept_suite.benchmark == suite.benchmark && going(id)
        });
        if running {
            return Ok(SuiteInsert::Running);
        }
        let open = kept
            .trials
            .iter()
            .filter(|((suite_id, _), run)| {
                kept.suites[suite_id].owner == owner
                    && runs.get(run).is_some_and(|run| run.end.is_none())
            })
            .count();
        let open = u32::try_from(open).unwrap_or(u32::MAX);
        if open + u32::from(suite.trials) > u32::from(MAX_OPEN_TRIALS) {
            return Ok(SuiteInsert::TooManyTrials(open));
        }
        kept.suites.entry(suite.id).or_insert_with(|| FakeSuite {
            owner,
            benchmark: suite.benchmark.to_string(),
            agents: suite.agents.clone(),
        });
        Ok(SuiteInsert::Inserted)
    }

    async fn insert_trial(
        &self,
        trial: &TrialKey,
        run_id: RunId,
        base_commit: &str,
    ) -> Result<(), FakeError> {
        if self.fails("insert_trial") {
            return Err(FakeError);
        }
        let mut kept = self.evaluation.lock().expect("evaluation");
        kept.trials.entry(trial_id(trial)).or_insert(run_id);
        kept.base_commits
            .entry(trial_id(trial))
            .or_insert_with(|| base_commit.to_string());
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
    /// Whether every call fails, as a model that is down.
    down: bool,
    verdicts: Mutex<VecDeque<Result<JudgeVerdict, FakeError>>>,
    asked: Mutex<Vec<(MacroUserIdStr<'static>, JudgeQuestion)>>,
}

impl FakeJudge {
    /// A judge whose every call fails.
    pub(crate) fn down() -> Self {
        Self {
            down: true,
            ..Self::default()
        }
    }

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
        if self.down {
            return Err(FakeError);
        }
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
