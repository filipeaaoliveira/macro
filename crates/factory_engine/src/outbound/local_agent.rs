//! A [`StepAgent`] for a machine where no agent can run.

use factory_records::domain::models::StepKey;
use macro_user_id::user_id::MacroUserIdStr;

use crate::domain::models::{AgentAnswer, AgentTask};
use crate::domain::ports::{Retryable, StepAgent};

/// The task gave the agent no outcome to end with.
#[derive(Debug, thiserror::Error)]
#[error("the task names no outcome to end with")]
pub struct NoOutcome;

impl Retryable for NoOutcome {
    fn retryable(&self) -> bool {
        false
    }
}

/// Does no work: ends every task with its first outcome and hands back the
/// prompt it was given. A Flow's agent Steps then run end to end, showing
/// what each was asked, on a machine with no agent runtime or model access.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalAgent;

impl StepAgent for LocalAgent {
    type Err = NoOutcome;

    #[tracing::instrument(err, skip_all, fields(run_id = %task.key.run_id, step = %task.step))]
    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, NoOutcome> {
        let outcome = task.outcomes.first().cloned().ok_or(NoOutcome)?;
        Ok(AgentAnswer {
            outcome,
            text: format!(
                "No agent ran this step: the local agent stands in for one. It was asked:\n\n{}",
                task.prompt
            ),
        })
    }

    async fn stop(&self, _key: StepKey, _owner: &MacroUserIdStr<'static>) -> Result<(), NoOutcome> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), NoOutcome> {
        Ok(())
    }
}
