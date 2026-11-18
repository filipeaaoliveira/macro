//! The Gate model on Macro's model router: one structured completion per
//! reply or stand-in question, on a small, cheap model, with its usage
//! recorded against the Run's owner and naming the Run. Each prompt is
//! read once (the Gate, the Run and the words differ from call to call), so
//! none is written to the prompt cache, whose write costs more than plain
//! input and would never be read back.

#[cfg(test)]
mod test;

use std::sync::Arc;

use agent::Message;
use agent::structured_output::{DynamicSchema, dynamic_structured_completion_uncached};
use ai_usage::{AiFeature, UsageContext, UsageRecorder};
use factory_records::domain::models::{Outcome, RunId};
use macro_user_id::user_id::MacroUserIdStr;
use serde::Deserialize;
use serde_json::json;

use crate::domain::gate_replies::UNCLEAR;
use crate::domain::ports::{GateModel, ModelPick, ModelQuestion};

/// Why a Gate model's call failed.
#[derive(Debug, thiserror::Error)]
pub enum GateModelError {
    /// The model could not be reached, or did not answer in the schema.
    #[error("the gate model's call failed: {0}")]
    Call(String),
}

/// The model's answer, as the schema asks it.
#[derive(Debug, Deserialize)]
struct Picked {
    choice: String,
    confidence: f64,
    reason: String,
}

/// The model a Gate model runs on unless told otherwise: Claude Haiku 5.5,
/// by the id Macro's model router knows.
pub const DEFAULT_GATE_MODEL: &str = "anthropic/claude-haiku-5-5";

/// A Gate model on Macro's model router.
#[derive(Clone)]
pub struct FastGateModel {
    model: String,
    recorder: Arc<dyn UsageRecorder>,
}

impl FastGateModel {
    /// The Gate model on `model`, a model id the router knows, such as
    /// [`DEFAULT_GATE_MODEL`]; its usage is recorded through `recorder`.
    pub fn new(model: String, recorder: Arc<dyn UsageRecorder>) -> Self {
        Self { model, recorder }
    }
}

impl GateModel for FastGateModel {
    type Err = GateModelError;

    #[tracing::instrument(err, skip_all, fields(model = %self.model, choices = question.choices.len()))]
    async fn pick(
        &self,
        owner: &MacroUserIdStr<'static>,
        run_id: RunId,
        question: &ModelQuestion,
    ) -> Result<ModelPick, GateModelError> {
        let mut choices: Vec<&str> = question.choices.iter().map(Outcome::as_str).collect();
        choices.push(UNCLEAR);
        let schema = DynamicSchema {
            name: "GateAnswer".to_string(),
            description: Some("The answer the text gives, how sure, and why.".to_string()),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["choice", "confidence", "reason"],
                "properties": {
                    "choice": { "type": "string", "enum": choices },
                    "confidence": { "type": "number", "minimum": 0, "maximum": 1 },
                    "reason": { "type": "string" }
                }
            }),
        };
        let value = dynamic_structured_completion_uncached(
            self.model.as_str(),
            question.instructions,
            vec![Message::user(question.prompt.clone())],
            schema,
            self.recorder.as_ref(),
            usage_context(owner, run_id),
        )
        .await
        .map_err(|error| GateModelError::Call(error.to_string()))?;
        let picked: Picked = serde_json::from_value(value)
            .map_err(|error| GateModelError::Call(error.to_string()))?;
        let choice = question
            .choices
            .iter()
            .find(|choice| choice.as_str() == picked.choice)
            .cloned();
        Ok(ModelPick {
            choice,
            confidence: picked.confidence,
            reason: picked.reason,
        })
    }
}

/// What a Gate model call's usage is recorded as: the factory's Gate model,
/// billed to `owner`, about the Run `run_id`, whose usage it adds to.
fn usage_context(owner: &MacroUserIdStr<'static>, run_id: RunId) -> UsageContext {
    UsageContext::new(AiFeature::FactoryGate, owner.clone()).with_entity(Some(run_id.as_uuid()))
}
