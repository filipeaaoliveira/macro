//! The task a Run files, as it ends, for each CI job its `ci` Steps
//! quarantined as flaky: as its owner, in its task's project when it has
//! one. A task's id is kept with its quarantine before it is filed, so an
//! end retried, or a filing that failed and is tried again at the end of
//! the same Run, files no second task.

use factory_records::domain::models::Run;
use factory_records::domain::ports::RunRepository;
use factory_records::domain::quality::{CiQuarantine, FLAKY_PERIOD};

use crate::domain::ports::{FollowUpMade, FollowUpTask, QuarantineTasks};

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
