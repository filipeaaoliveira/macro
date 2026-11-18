//! The in-memory stand-in for marking Runs flawed.

use chrono::Utc;
use factory_records::domain::flaws::{FlawMarking, FlawRepository};
use factory_records::domain::models::{FlawMarker, RunFlaw, RunId, RunOutcome};

use super::{FakeError, FakeRepo};

impl FlawRepository for FakeRepo {
    type Err = FakeError;

    async fn mark_flawed(
        &self,
        id: RunId,
        by: &FlawMarker,
        reason: &str,
    ) -> Result<FlawMarking, FakeError> {
        let mut runs = self.runs.lock().expect("runs");
        let Some(run) = runs.get_mut(&id) else {
            return Ok(FlawMarking::NoSuchRun);
        };
        // As the records check: a person's mark names the Run's owner.
        if let FlawMarker::Person { user_id } = by
            && *user_id != run.user_id
        {
            return Err(FakeError);
        }
        let Some(end) = run.end.as_mut() else {
            return Ok(FlawMarking::NotSucceeded(None));
        };
        match end.outcome {
            RunOutcome::Success => {
                end.outcome = RunOutcome::Flawed;
                end.flaw = Some(RunFlaw {
                    by: by.clone(),
                    reason: reason.to_string(),
                    marked_at: Utc::now(),
                });
                Ok(FlawMarking::Flawed(end.clone()))
            }
            RunOutcome::Flawed => Ok(FlawMarking::Flawed(end.clone())),
            outcome => Ok(FlawMarking::NotSucceeded(Some(outcome))),
        }
    }
}
