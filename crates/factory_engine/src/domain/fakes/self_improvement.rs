//! Failures and self-improvement Runs in memory, with the same rules as the
//! Postgres repository: one open self-improvement Run per team, failures
//! kept once per Run and source.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use factory_records::domain::models::RunId;
use factory_records::domain::self_improvement::{
    DefinitionNaming, Failure, FailureSource, Improvement, ImprovementBegin, ImprovementState,
    NewFailure, PendingFailures, RunFailureFacts, SelfImprovementRepository, Settlement,
};
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FakeError, FakeRepo};

/// A failure kept, with its team, Definition and state.
#[derive(Debug, Clone)]
pub(crate) struct FakeFailure {
    pub(crate) team_id: Uuid,
    pub(crate) repo_url: String,
    pub(crate) failure: Failure,
    pub(crate) improvement: Option<Uuid>,
    pub(crate) addressed: bool,
}

/// What [`FakeRepo`] keeps of failures and self-improvement Runs.
#[derive(Debug, Clone, Default)]
pub(crate) struct FakeImprovement {
    /// The failures, in the order they were kept.
    pub(crate) failures: Vec<FakeFailure>,
    /// The self-improvement Runs, in the order they began.
    pub(crate) improvements: Vec<Improvement>,
}

/// A failure's key: its Run and source.
fn same(a: &Failure, run_id: RunId, source: &FailureSource) -> bool {
    a.run_id == run_id
        && a.source.label() == source.label()
        && matches!(
            (&a.source, source),
            (FailureSource::Flawed, FailureSource::Flawed)
                | (FailureSource::Scorer { .. }, FailureSource::Scorer { .. })
        )
}

impl FakeRepo {
    /// Keep a failure as kept at `failed_at`, for tests that need an old one.
    pub(crate) fn keep_failure_at(
        &self,
        team_id: Uuid,
        repo_url: &str,
        run_id: RunId,
        failure: NewFailure,
        failed_at: DateTime<Utc>,
    ) {
        let flow = self
            .runs
            .lock()
            .expect("runs")
            .get(&run_id)
            .and_then(|run| run.flow.clone())
            .unwrap_or_default();
        self.improvement
            .lock()
            .expect("improvement")
            .failures
            .push(FakeFailure {
                team_id,
                repo_url: repo_url.to_string(),
                failure: Failure {
                    run_id,
                    flow,
                    source: failure.source,
                    reason: failure.reason,
                    failed_at,
                },
                improvement: None,
                addressed: false,
            });
    }
}

impl SelfImprovementRepository for FakeRepo {
    type Err = FakeError;

    async fn definition_naming(
        &self,
        team_id: Uuid,
    ) -> Result<Option<DefinitionNaming>, FakeError> {
        let repo_url = self
            .team_definitions
            .lock()
            .expect("team definitions")
            .get(&team_id)
            .cloned();
        let named_by = self
            .definition_namers
            .lock()
            .expect("definition namers")
            .get(&team_id)
            .cloned();
        Ok(repo_url.map(|repo_url| DefinitionNaming {
            repo_url,
            named_by: named_by.and_then(|id| MacroUserIdStr::try_from(id).ok()),
        }))
    }

    async fn failure_facts(&self, run_id: RunId) -> Result<Option<RunFailureFacts>, FakeError> {
        let Some(run) = self.runs.lock().expect("runs").get(&run_id).cloned() else {
            return Ok(None);
        };
        let judgments = self
            .evaluation
            .lock()
            .expect("evaluation")
            .run_scores
            .get(&run_id)
            .map(|judgments| {
                judgments
                    .iter()
                    .map(|judgment| {
                        (
                            judgment.scorer.clone(),
                            judgment.score,
                            judgment.reason.clone(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let definition_evaluation = self
            .definition_evaluations
            .lock()
            .expect("definition evaluations")
            .get(&run_id)
            .cloned();
        let flow = run
            .flow
            .clone()
            .or_else(|| self.flows.lock().expect("flows").get(&run_id).cloned());
        Ok(Some(RunFailureFacts {
            user_id: run.user_id.clone(),
            flow: flow.unwrap_or_default(),
            repo_url: run
                .repository
                .as_ref()
                .map(|repository| repository.url.clone()),
            definition_repo_url: run.definition.as_ref().map(|kept| kept.repo_url.clone()),
            definition_evaluation,
            outcome: run.end.as_ref().map(|end| end.outcome),
            flaw_reason: run
                .end
                .as_ref()
                .and_then(|end| end.flaw.as_ref())
                .map(|flaw| flaw.reason.clone()),
            judgments,
        }))
    }

    async fn record_failures(
        &self,
        team_id: Uuid,
        repo_url: &str,
        run_id: RunId,
        failures: &[NewFailure],
    ) -> Result<(), FakeError> {
        for failure in failures {
            let kept = self
                .improvement
                .lock()
                .expect("improvement")
                .failures
                .iter()
                .any(|kept| same(&kept.failure, run_id, &failure.source));
            if !kept {
                self.keep_failure_at(team_id, repo_url, run_id, failure.clone(), Utc::now());
            }
        }
        Ok(())
    }

    async fn pending_failures(
        &self,
        team_id: Uuid,
        repo_url: &str,
    ) -> Result<PendingFailures, FakeError> {
        let kept = self.improvement.lock().expect("improvement");
        let mut pending = PendingFailures::default();
        let mut by_source: BTreeMap<String, u32> = BTreeMap::new();
        for failure in kept
            .failures
            .iter()
            .filter(|kept| kept.team_id == team_id && kept.repo_url == repo_url)
            .filter(|kept| kept.improvement.is_none())
        {
            pending.count += 1;
            pending.oldest = Some(match pending.oldest {
                Some(oldest) => oldest.min(failure.failure.failed_at),
                None => failure.failure.failed_at,
            });
            *by_source
                .entry(failure.failure.source.label().to_string())
                .or_default() += 1;
        }
        let mut by_source: Vec<(String, u32)> = by_source.into_iter().collect();
        by_source.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        pending.by_source = by_source;
        Ok(pending)
    }

    async fn latest_improvement(&self, team_id: Uuid) -> Result<Option<Improvement>, FakeError> {
        Ok(self
            .improvement
            .lock()
            .expect("improvement")
            .improvements
            .iter()
            .rev()
            .find(|improvement| improvement.team_id == team_id)
            .cloned())
    }

    async fn recent_improvements(
        &self,
        team_id: Uuid,
        limit: u32,
    ) -> Result<Vec<Improvement>, FakeError> {
        Ok(self
            .improvement
            .lock()
            .expect("improvement")
            .improvements
            .iter()
            .rev()
            .filter(|improvement| improvement.team_id == team_id)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn begin_improvement(
        &self,
        id: Uuid,
        team_id: Uuid,
        repo_url: &str,
        limit: u32,
    ) -> Result<ImprovementBegin, FakeError> {
        let mut kept = self.improvement.lock().expect("improvement");
        let open = kept
            .improvements
            .iter()
            .any(|improvement| improvement.team_id == team_id && improvement.state.is_open());
        if open {
            return Ok(ImprovementBegin::AlreadyOpen);
        }
        let mut pending: Vec<&mut FakeFailure> = kept
            .failures
            .iter_mut()
            .filter(|kept| {
                kept.team_id == team_id && kept.repo_url == repo_url && kept.improvement.is_none()
            })
            .collect();
        if pending.is_empty() {
            return Ok(ImprovementBegin::NoFailures);
        }
        pending.sort_by_key(|kept| kept.failure.failed_at);
        let mut claimed = Vec::new();
        for failure in pending.into_iter().take(limit as usize) {
            failure.improvement = Some(id);
            claimed.push(failure.failure.clone());
        }
        kept.improvements.push(Improvement {
            id,
            team_id,
            repo_url: repo_url.to_string(),
            run_id: None,
            state: ImprovementState::Starting,
            refusal: None,
            created_at: Utc::now(),
            settled_at: None,
            failures: u32::try_from(claimed.len()).unwrap_or(u32::MAX),
        });
        Ok(ImprovementBegin::Claimed(claimed))
    }

    async fn link_improvement_run(&self, id: Uuid, run_id: RunId) -> Result<bool, FakeError> {
        let mut kept = self.improvement.lock().expect("improvement");
        let Some(improvement) = kept
            .improvements
            .iter_mut()
            .find(|improvement| improvement.id == id)
        else {
            return Ok(false);
        };
        if improvement.state != ImprovementState::Starting {
            return Ok(false);
        }
        improvement.run_id = Some(run_id);
        improvement.state = ImprovementState::Open;
        Ok(true)
    }

    async fn settle_improvement(&self, id: Uuid, settlement: &Settlement) -> Result<(), FakeError> {
        let mut kept = self.improvement.lock().expect("improvement");
        let Some(improvement) = kept
            .improvements
            .iter_mut()
            .find(|improvement| improvement.id == id && improvement.state.is_open())
        else {
            return Ok(());
        };
        let (state, refusal) = match settlement {
            Settlement::Merged => (ImprovementState::Merged, None),
            Settlement::Closed => (ImprovementState::Closed, None),
            Settlement::Refused(why) => (ImprovementState::Refused, Some(why.clone())),
        };
        improvement.state = state;
        improvement.refusal = refusal;
        improvement.settled_at = Some(Utc::now());
        for failure in kept
            .failures
            .iter_mut()
            .filter(|failure| failure.improvement == Some(id) && !failure.addressed)
        {
            if state == ImprovementState::Merged {
                failure.addressed = true;
            } else {
                failure.improvement = None;
            }
        }
        Ok(())
    }
}
