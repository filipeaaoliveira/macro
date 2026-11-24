//! The task a Run files, as it ends, for each CI job its `ci` Steps
//! quarantined as flaky: as its owner, in its task's project when it has
//! one. A task's id is kept with its quarantine before it is filed, so an
//! end retried, or a filing that failed and is tried again at the end of
//! the same Run, files no second task.
//!
//! A person quarantines a CI job by hand, or releases it, from Insights: a
//! team's owner or admin, in their own signed-in session, in a repository
//! they may push to. A quarantine by hand covers every failure of the job
//! and files no task.

use chrono::Utc;
use factory_records::domain::automations::AutomationRepository;
use factory_records::domain::board::RunBoardRepository;
use factory_records::domain::evaluation::EvaluationRepository;
use factory_records::domain::flaws::FlawRepository;
use factory_records::domain::main_hold::MainHoldRepository;
use factory_records::domain::models::Run;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::quality::{CI_NAME_BYTES, CiQuarantine, FLAKY_PERIOD};
use factory_records::domain::review_rounds::ReviewRoundRepository;
use factory_records::domain::self_improvement::SelfImprovementRepository;
use factory_records::domain::standing_acts::StandingActRepository;
use factory_records::domain::standing_rules::StandingRulesRepository;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::{FactoryServiceImpl, internal};
use crate::domain::models::{CiQuarantineChange, FactoryError};
use crate::domain::ports::{
    FollowUpMade, FollowUpTask, PullRequests, QuarantineTasks, RunWorkflows, StepExecutor,
    TaskDirectory, UserDirectory,
};

impl<R, W, E, U, T, G> FactoryServiceImpl<R, W, E, U, T, G>
where
    R: RunRepository
        + MainHoldRepository
        + ReviewRoundRepository
        + StandingRulesRepository
        + AutomationRepository
        + FlawRepository
        + RunBoardRepository
        + EvaluationRepository
        + StandingActRepository
        + SelfImprovementRepository,
    W: RunWorkflows,
    E: StepExecutor,
    U: UserDirectory,
    T: TaskDirectory,
    G: PullRequests,
{
    /// The repository `repo_url` names, by its URL in lower case, once
    /// `actor` may change its quarantines: they own or administer their
    /// team, and may push to the repository. `job` must name a CI job.
    async fn quarantine_scope(
        &self,
        actor: &MacroUserIdStr<'static>,
        repo_url: &str,
        job: &str,
    ) -> Result<String, FactoryError> {
        if job.is_empty() || job.len() > CI_NAME_BYTES {
            return Err(FactoryError::InvalidCiJob { max: CI_NAME_BYTES });
        }
        self.administered_team(actor).await?;
        let repository = self.reachable_repository(actor, repo_url).await?;
        Ok(repository.url.to_lowercase())
    }

    /// Quarantine `job` in `repo_url` by hand, as
    /// [`crate::domain::ports::FactoryService::quarantine_ci_job`] does.
    #[tracing::instrument(skip(self, actor))]
    pub(super) async fn quarantine_job(
        &self,
        actor: &MacroUserIdStr<'static>,
        repo_url: &str,
        job: &str,
    ) -> Result<CiQuarantineChange, FactoryError> {
        let repository = self.quarantine_scope(actor, repo_url, job).await?;
        let changed = self
            .repo
            .quarantine_ci_job(
                &repository,
                job,
                actor,
                Uuid::now_v7(),
                Utc::now() - FLAKY_PERIOD,
            )
            .await
            .map_err(internal)?;
        Ok(CiQuarantineChange {
            repository,
            job: job.to_string(),
            changed,
        })
    }

    /// Release `job` in `repo_url`, as
    /// [`crate::domain::ports::FactoryService::release_ci_job`] does.
    #[tracing::instrument(skip(self, actor))]
    pub(super) async fn release_job(
        &self,
        actor: &MacroUserIdStr<'static>,
        repo_url: &str,
        job: &str,
    ) -> Result<CiQuarantineChange, FactoryError> {
        let repository = self.quarantine_scope(actor, repo_url, job).await?;
        let lifted = self
            .repo
            .lift_ci_job_quarantines(&repository, job)
            .await
            .map_err(internal)?;
        Ok(CiQuarantineChange {
            repository,
            job: job.to_string(),
            changed: lifted > 0,
        })
    }
}

/// Where `quarantine`'s job failed, for a person: its step and its test.
fn failure_words(quarantine: &CiQuarantine) -> String {
    let mut words = String::new();
    if let Some(step) = &quarantine.failure.failed_step {
        words.push_str(&format!(" in the step `{step}`"));
    }
    if let Some(test) = &quarantine.failure.failure {
        words.push_str(&format!(" on `{test}`"));
    }
    words
}

/// The task filed for `quarantine`, which `run` made.
fn quarantine_task(run: &Run, quarantine: &CiQuarantine) -> FollowUpTask {
    FollowUpTask {
        name: format!("Fix the flaky CI job {}", quarantine.failure.name),
        markdown: format!(
            "The CI job `{name}` in {repository} failed{how} and then passed when it ran \
             again {passed} times in {days} days, so the factory quarantined that failure: \
             a `ci` Step now reruns it once when it alone turns a pull request red, before \
             any CI fix.\n\nFind why it fails only some of the time, and fix it. Factory \
             Run {run_id} quarantined it.",
            name = quarantine.failure.name,
            how = failure_words(quarantine),
            repository = quarantine.repository,
            passed = quarantine.passed_retries,
            days = FLAKY_PERIOD.num_days(),
            run_id = run.id,
        ),
    }
}

/// File a task for each quarantine `run` made that has none yet. It never
/// fails the Run's end: a task that cannot be filed now is logged, and its
/// quarantine stays listed without one.
pub(crate) async fn file_tasks<R: RunRepository, Q: QuarantineTasks>(
    repo: &R,
    tasks: &Q,
    run: &Run,
) {
    let quarantines = match repo.unfiled_ci_quarantines(run.id).await {
        Ok(quarantines) => quarantines,
        Err(error) => {
            tracing::error!(%error, "the run's quarantined ci jobs could not be read");
            return;
        }
    };
    for quarantine in quarantines {
        let task = quarantine_task(run, &quarantine);
        let filed = tasks
            .file_quarantine(&run.user_id, run.task_id, &task, quarantine.task_id)
            .await;
        match filed {
            Ok(Some(FollowUpMade::Made | FollowUpMade::OtherId(_))) => {
                let marked = repo.mark_ci_quarantine_filed(quarantine.task_id).await;
                if let Err(error) = marked {
                    tracing::error!(%error, job = quarantine.failure.name, "a filed quarantine task could not be kept");
                }
            }
            Ok(Some(FollowUpMade::Refused)) => {
                tracing::error!(
                    job = quarantine.failure.name,
                    "the quarantine task was refused"
                );
            }
            Ok(None) => {}
            Err(error) => {
                tracing::error!(%error, job = quarantine.failure.name, "the quarantine task could not be filed");
            }
        }
    }
}
