//! Experiment arms, Scorers' judgments and Benchmark suites in memory, with
//! the same first-write-wins rules as the Postgres repository.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use factory_records::domain::evaluation::{
    AgentChoices, CalibrationTally, EvaluationRecords, EvaluationRepository, Judgment,
    MAX_OPEN_TRIALS, NewSuite, PASS_SCORE, RunAgentRule, RunAgents, RunArm, RunJudgment,
    ScorerJudge, ScorerLabel, ScoringState, ScoringStep, ScoringSubject, SubjectTrial, SuiteInsert,
    TrialKey, TrialResult,
};
use factory_records::domain::insights::InsightsScope;
use factory_records::domain::models::RunId;
use factory_records::domain::ports::RunRepository as _;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use crate::domain::ports::{JudgeQuestion, JudgeVerdict, RunJudge};

use super::{FakeError, FakeRepo};

/// A suite kept: whose, its Benchmark and configuration.
#[derive(Debug, Clone)]
pub(crate) struct FakeSuite {
    pub(crate) planned: u16,
    pub(crate) owner: String,
    pub(crate) benchmark: String,
    pub(crate) agents: AgentChoices,
}

/// What [`FakeRepo`] keeps of Experiments, Scorers and Benchmarks.
#[derive(Debug, Clone, Default)]
pub(crate) struct FakeEvaluation {
    /// Each Run's arms.
    pub(crate) arms: BTreeMap<RunId, Vec<RunArm>>,
    /// Each Run's Agent Rules, one per Role, by Role.
    pub(crate) agent_rules: BTreeMap<RunId, Vec<RunAgentRule>>,
    /// The Runs that started with their task's tags unread.
    pub(crate) task_tags_unread: std::collections::BTreeSet<RunId>,
    /// The live Scorers' judgments, by Run.
    pub(crate) run_scores: BTreeMap<RunId, Vec<Judgment>>,
    /// The owners' labels of the live judgments, by Run and Scorer.
    pub(crate) labels: BTreeMap<(RunId, String), ScorerLabel>,
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
    /// When each Run's scoring finished.
    pub(crate) scored: BTreeMap<RunId, DateTime<Utc>>,
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

    async fn record_agent_rules(
        &self,
        run_id: RunId,
        rules: &[RunAgentRule],
    ) -> Result<(), FakeError> {
        if self.fails("record_agent_rules") {
            return Err(FakeError);
        }
        let mut kept = self.evaluation.lock().expect("evaluation");
        for rule in rules {
            let run = kept.agent_rules.entry(run_id).or_default();
            if !run.iter().any(|kept| kept.role == rule.role) {
                run.push(rule.clone());
                run.sort_by(|first, second| first.role.cmp(&second.role));
            }
        }
        Ok(())
    }

    async fn record_task_tags_unread(&self, run_id: RunId) -> Result<(), FakeError> {
        if self.fails("record_task_tags_unread") {
            return Err(FakeError);
        }
        let mut kept = self.evaluation.lock().expect("evaluation");
        kept.task_tags_unread.insert(run_id);
        Ok(())
    }

    async fn runs_task_tags_unread(
        &self,
        run_ids: &[RunId],
    ) -> Result<std::collections::BTreeSet<RunId>, FakeError> {
        let kept = self.evaluation.lock().expect("evaluation");
        Ok(run_ids
            .iter()
            .filter(|run_id| kept.task_tags_unread.contains(run_id))
            .copied()
            .collect())
    }

    async fn runs_agent_rules(
        &self,
        run_ids: &[RunId],
    ) -> Result<BTreeMap<RunId, Vec<RunAgentRule>>, FakeError> {
        let kept = self.evaluation.lock().expect("evaluation");
        Ok(run_ids
            .iter()
            .filter_map(|run_id| Some((*run_id, kept.agent_rules.get(run_id)?.clone())))
            .collect())
    }

    async fn run_agents(&self, run_id: RunId) -> Result<RunAgents, FakeError> {
        let kept = self.evaluation.lock().expect("evaluation");
        let mut agents = RunAgents::default();
        for rule in kept.agent_rules.get(&run_id).into_iter().flatten() {
            agents.ruled.insert(rule.role.clone(), rule.agent.clone());
        }
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

    async fn set_planned_trials(&self, suite_id: Uuid, trials: u16) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        if let Some(suite) = kept.suites.get_mut(&suite_id) {
            suite.planned = trials;
        }
        Ok(())
    }

    async fn delete_suite(&self, suite_id: Uuid) -> Result<(), FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        if !kept.trials.keys().any(|(suite, _)| *suite == suite_id) {
            kept.suites.remove(&suite_id);
        }
        Ok(())
    }

    async fn scoring_subject(&self, run_id: RunId) -> Result<Option<ScoringSubject>, FakeError> {
        // Read before any lock is held: the read takes its own.
        let used = self.step_sessions(run_id).await?;
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
            branch_of: run.branch_of(),
            user_id: run.user_id,
            flow,
            request: run.request,
            repo_url: run
                .repository
                .as_ref()
                .map(|repository| repository.url.clone()),
            base_branch: run.repository.map(|repository| repository.base_branch),
            outcome: run.end.map(|end| end.outcome),
            steps,
            sessions: used,
            trial,
            judged,
            evaluation: self
                .definition_evaluations
                .lock()
                .expect("definition evaluations")
                .get(&run_id)
                .cloned(),
        }))
    }

    async fn scoring_state(&self, run_id: RunId) -> Result<Option<ScoringState>, FakeError> {
        let Some(run) = self.runs.lock().expect("runs").get(&run_id).cloned() else {
            return Ok(None);
        };
        let flow = self.flows.lock().expect("flows").get(&run_id).cloned();
        let evaluation = self
            .definition_evaluations
            .lock()
            .expect("definition evaluations")
            .get(&run_id)
            .cloned();
        let kept = self.evaluation.lock().expect("evaluation");
        Ok(Some(ScoringState {
            flow,
            outcome: run.end.as_ref().map(|end| end.outcome),
            ended_at: run.end.as_ref().map(|end| end.ended_at),
            scored_at: kept.scored.get(&run_id).copied(),
            trial: kept.trial_of(run_id).is_some(),
            judged: kept
                .run_scores
                .get(&run_id)
                .into_iter()
                .flatten()
                .map(|judgment| judgment.scorer.clone())
                .collect(),
            evaluation,
        }))
    }

    async fn mark_run_scored(&self, run_id: RunId) -> Result<(), FakeError> {
        self.evaluation
            .lock()
            .expect("evaluation")
            .scored
            .entry(run_id)
            .or_insert_with(Utc::now);
        Ok(())
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
            planned: suite.trials,
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

    /// Judgments keep no time here: each reads as judged at the epoch.
    async fn run_judgments(&self, run_id: RunId) -> Result<Vec<RunJudgment>, FakeError> {
        let kept = self.evaluation.lock().expect("evaluation");
        let mut judgments: Vec<RunJudgment> = kept
            .run_scores
            .get(&run_id)
            .into_iter()
            .flatten()
            .map(|judgment| RunJudgment {
                scorer: judgment.scorer.clone(),
                score: judgment.score,
                reason: judgment.reason.clone(),
                model: judgment.model.clone(),
                instructions_digest: Some(judgment.instructions_digest.clone()),
                judged_at: DateTime::<Utc>::UNIX_EPOCH,
                label: kept.labels.get(&(run_id, judgment.scorer.clone())).copied(),
            })
            .collect();
        judgments.sort_by(|first, second| first.scorer.cmp(&second.scorer));
        Ok(judgments)
    }

    async fn label_judgment(
        &self,
        run_id: RunId,
        scorer: &str,
        passed: bool,
    ) -> Result<Option<ScorerLabel>, FakeError> {
        let mut kept = self.evaluation.lock().expect("evaluation");
        let judged = kept
            .run_scores
            .get(&run_id)
            .is_some_and(|scores| scores.iter().any(|judgment| judgment.scorer == scorer));
        if !judged {
            return Ok(None);
        }
        let label = ScorerLabel {
            passed,
            labelled_at: Utc::now(),
        };
        kept.labels.insert((run_id, scorer.to_string()), label);
        Ok(Some(label))
    }

    async fn calibrations(
        &self,
        user_id: &MacroUserIdStr<'static>,
        judges: &[ScorerJudge],
    ) -> Result<Vec<CalibrationTally>, FakeError> {
        let runs = self.runs.lock().expect("runs").clone();
        let kept = self.evaluation.lock().expect("evaluation");
        let mut tallies: BTreeMap<String, CalibrationTally> = BTreeMap::new();
        for ((run_id, scorer), label) in &kept.labels {
            let owned = runs.get(run_id).is_some_and(|run| run.user_id == *user_id);
            let judgment = kept
                .run_scores
                .get(run_id)
                .and_then(|scores| scores.iter().find(|judgment| judgment.scorer == *scorer));
            let (true, Some(judgment)) = (owned, judgment) else {
                continue;
            };
            let by_current_judge = judges.iter().any(|judge| {
                judge.scorer == judgment.scorer
                    && judge.model == judgment.model
                    && judge.instructions_digest == judgment.instructions_digest
            });
            if !by_current_judge {
                continue;
            }
            let tally = tallies
                .entry(scorer.clone())
                .or_insert_with(|| CalibrationTally {
                    scorer: scorer.clone(),
                    labelled: 0,
                    agreed: 0,
                });
            tally.labelled += 1;
            if (judgment.score >= PASS_SCORE) == label.passed {
                tally.agreed += 1;
            }
        }
        Ok(tallies.into_values().collect())
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
