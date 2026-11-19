//! The activities' work over a stand-in for Temporal's activity context,
//! which the test cancels and which counts heartbeats.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_records::domain::models::{EntryId, Role, RunId, StepKey};
use temporalio_sdk::activities::ActivityError;
use tokio::sync::{Notify, watch};

use super::{AGENT_OVERDUE_FAILURE, FactoryActivities, INFRASTRUCTURE_FAILURE, Liveness};
use crate::domain::fakes::{FakeGithub, FakeRepo};
use crate::domain::flow::StepName;
use crate::domain::gate::{GateAnswered, GateOpening, GateTelling};
use crate::domain::models::{RunFinish, StepEnding, StepError, StepReport, StepRequest, StepStart};
use crate::domain::ports::StepService;
use crate::domain::review_rounds::ReviewRoundsImpl;

/// An activity context the test cancels, which counts its heartbeats.
struct Context {
    cancelled: watch::Sender<bool>,
    beats: AtomicUsize,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            cancelled: watch::Sender::new(false),
            beats: AtomicUsize::new(0),
        }
    }
}

impl Liveness for Context {
    fn cancelled(&self) -> impl Future<Output = ()> + Send {
        let mut cancelled = self.cancelled.subscribe();
        async move {
            // The sender lives as long as the context.
            let _ = cancelled.wait_for(|cancelled| *cancelled).await;
        }
    }

    async fn beat(&self) {
        self.beats.fetch_add(1, Ordering::SeqCst);
    }
}

/// A Step service whose work never ends, and whose stop takes `stop_takes`.
/// It says when the work starts, and counts the stops, and whether the work
/// is still held.
#[derive(Clone, Default)]
struct Steps {
    stops: Arc<AtomicUsize>,
    stop_takes: Duration,
    working: Arc<Mutex<bool>>,
    started: Arc<Notify>,
}

/// Clears `working` when the work is dropped.
struct Working(Arc<Mutex<bool>>);

impl Drop for Working {
    fn drop(&mut self) {
        *self.0.lock().expect("working") = false;
    }
}

impl StepService for Steps {
    async fn start_step(&self, _request: StepRequest) -> Result<StepStart, StepError> {
        Ok(StepStart::Started)
    }

    async fn run_step(&self, _request: StepRequest) -> Result<StepReport, StepError> {
        *self.working.lock().expect("working") = true;
        let _working = Working(self.working.clone());
        self.started.notify_one();
        std::future::pending().await
    }

    async fn stop_step(&self, _request: StepRequest) -> Result<(), StepError> {
        tokio::time::sleep(self.stop_takes).await;
        self.stops.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn release_step(&self, _request: StepRequest) -> Result<(), StepError> {
        Ok(())
    }

    async fn end_step(&self, _ending: StepEnding) -> Result<(), StepError> {
        Ok(())
    }

    async fn open_gate(&self, _opening: GateOpening) -> Result<(), StepError> {
        Ok(())
    }

    async fn answer_gate(&self, _answered: GateAnswered) -> Result<(), StepError> {
        Ok(())
    }

    async fn rule_gate(
        &self,
        _ruling: crate::domain::gate::GateRuling,
    ) -> Result<Option<crate::domain::standing_rules::RuleAnswer>, StepError> {
        Ok(None)
    }

    async fn notify_gate(&self, _key: StepKey) -> Result<(), StepError> {
        Ok(())
    }

    async fn tell_gate(&self, _telling: GateTelling) -> Result<(), StepError> {
        Ok(())
    }

    async fn end_run(&self, _finish: RunFinish) -> Result<(), StepError> {
        Ok(())
    }

    async fn check_budget(
        &self,
        _check: crate::domain::token_budget::BudgetCheck,
    ) -> Result<crate::domain::token_budget::BudgetVerdict, StepError> {
        Ok(crate::domain::token_budget::BudgetVerdict::Within)
    }
}

fn plan_request() -> StepRequest {
    StepRequest {
        key: StepKey {
            run_id: RunId::generate(),
            seq: 1,
        },
        entry: EntryId::try_from("plan".to_string()).expect("valid"),
        step: StepName::try_from("plan".to_string()).expect("valid"),
        role: Some(Role::try_from("planner".to_string()).expect("valid")),
        visit: 1,
        round: None,
        may_ask: false,
        questions_left: 0,
        resumes: None,
    }
}

/// Review rounds over records with no Run: these tests open none.
fn no_reviews() -> ReviewRoundsImpl<FakeRepo, FakeGithub, FakeRepo> {
    ReviewRoundsImpl::new(
        FakeRepo::default(),
        FakeGithub::default(),
        FakeRepo::default(),
    )
}

/// The activity can't tell the Run's cancellation from the worker shutting
/// down or an attempt timed out: it drops the work and leaves the agent be,
/// for a retry to go on with, or the workflow to stop.
#[tokio::test(start_paused = true)]
async fn a_cancelled_step_drops_its_work_and_leaves_its_agent_be() {
    let steps = Steps::default();
    let activities = FactoryActivities::new(steps.clone(), no_reviews());
    let context = Context::default();

    let work = activities.work(&context, plan_request());
    let cancel = async {
        steps.started.notified().await;
        assert!(*steps.working.lock().expect("working"));
        context.cancelled.send_replace(true);
    };
    let (worked, ()) = tokio::join!(work, cancel);

    assert!(
        matches!(worked, Err(ActivityError::Cancelled { .. })),
        "{worked:?}"
    );
    assert!(!*steps.working.lock().expect("working"));
    assert_eq!(steps.stops.load(Ordering::SeqCst), 0);
}

/// Stopping an agent watches its session for a while: Temporal must keep
/// hearing from the worker all along.
#[tokio::test(start_paused = true)]
async fn stopping_a_step_heartbeats_while_it_goes() {
    let steps = Steps {
        stop_takes: Duration::from_secs(70),
        ..Steps::default()
    };
    let activities = FactoryActivities::new(steps.clone(), no_reviews());
    let context = Context::default();

    activities
        .stop(&context, plan_request())
        .await
        .expect("stop");

    assert_eq!(steps.stops.load(Ordering::SeqCst), 1);
    assert!(context.beats.load(Ordering::SeqCst) >= 4);
}

/// Work whose agents' quota ran out fails for good as infrastructure's, for
/// the workflow to read; work that fails for itself carries no such type.
#[test]
fn a_quota_that_ran_out_fails_as_infrastructure() {
    let out_of_quota = super::failure(StepError::OutOfQuota(rootcause::report!("no quota")));
    let for_good = super::failure(StepError::Final(rootcause::report!("bad outcome")));

    let ActivityError::Application(out_of_quota) = out_of_quota else {
        panic!("not an application failure: {out_of_quota:?}");
    };
    assert!(out_of_quota.is_non_retryable());
    assert_eq!(out_of_quota.type_name(), Some(INFRASTRUCTURE_FAILURE));
    let ActivityError::Application(for_good) = for_good else {
        panic!("not an application failure: {for_good:?}");
    };
    assert!(for_good.is_non_retryable());
    assert_eq!(for_good.type_name(), None);
}

/// An agent still at work when the wait for it ended is retried, and its
/// failure says so, for a Step whose time runs out on it to read as the
/// work's.
#[test]
fn an_overdue_agent_is_retried_as_the_work_s() {
    let overdue = super::failure(StepError::Overdue(rootcause::report!("still at work")));

    let ActivityError::Application(overdue) = overdue else {
        panic!("not an application failure: {overdue:?}");
    };
    assert!(!overdue.is_non_retryable());
    assert_eq!(overdue.type_name(), Some(AGENT_OVERDUE_FAILURE));
}
