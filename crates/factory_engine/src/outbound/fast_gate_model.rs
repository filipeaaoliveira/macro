//! The Gate model on Macro's model router: one structured completion per
//! reply or stand-in question, on a small, cheap model, with its usage
//! recorded against the Run's owner and naming the Run. Each prompt is
//! read once (the Gate, the Run and the words differ from call to call), so
//! none is written to the prompt cache, whose write costs more than plain
//! input and would never be read back.

#[cfg(test)]
mod test;

use std::sync::Arc;

use agent::structured_output::{
    DynamicSchema, dynamic_structured_completion, dynamic_structured_completion_uncached,
};
use agent::{Message, PromptCache};
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

/// One structured completion the Gate model asks for: what the model is
/// told, what it reads, the schema it answers in, whether the prompt is
/// written to the prompt cache, and what its usage is recorded as.
#[derive(Debug)]
pub struct StructuredCall {
    /// The model, by the id Macro's model router knows.
    pub model: String,
    /// What the model is told to do.
    pub instructions: &'static str,
    /// What it reads.
    pub prompt: String,
    /// The schema its answer must match.
    pub schema: DynamicSchema,
    /// Whether the prompt is written to the prompt cache.
    pub cache: PromptCache,
    /// What its usage is recorded as.
    pub usage: UsageContext,
}

/// Where a Gate model's structured completions are made: Macro's model
/// router ([`RouterCompletion`]).
pub trait StructuredCompletion: Send + Sync + 'static {
    /// Make `call` and return the model's answer.
    fn complete(
        &self,
        call: StructuredCall,
    ) -> impl Future<Output = Result<serde_json::Value, GateModelError>> + Send;
}

/// Structured completions on Macro's model router, their usage recorded
/// through `recorder`.
#[derive(Clone)]
pub struct RouterCompletion {
    recorder: Arc<dyn UsageRecorder>,
}

impl StructuredCompletion for RouterCompletion {
    async fn complete(&self, call: StructuredCall) -> Result<serde_json::Value, GateModelError> {
        let messages = vec![Message::user(call.prompt)];
        let recorder = self.recorder.as_ref();
        let answered = match call.cache {
            PromptCache::Write => {
                dynamic_structured_completion(
                    call.model,
                    call.instructions,
                    messages,
                    call.schema,
                    recorder,
                    call.usage,
                )
                .await
            }
            PromptCache::Skip => {
                dynamic_structured_completion_uncached(
                    call.model,
                    call.instructions,
                    messages,
                    call.schema,
                    recorder,
                    call.usage,
                )
                .await
            }
        };
        answered.map_err(|error| GateModelError::Call(error.to_string()))
    }
}

/// A Gate model on Macro's model router, or on `completion` in tests.
#[derive(Clone)]
pub struct FastGateModel<C = RouterCompletion> {
    model: String,
    completion: C,
}

impl FastGateModel {
    /// The Gate model on `model`, a model id the router knows, such as
    /// [`DEFAULT_GATE_MODEL`]; its usage is recorded through `recorder`.
    pub fn new(model: String, recorder: Arc<dyn UsageRecorder>) -> Self {
        Self {
            model,
            completion: RouterCompletion { recorder },
        }
    }
}

impl<C> FastGateModel<C> {
    /// The Gate model on `model`, making its completions on `completion`.
    pub fn with_completion(model: String, completion: C) -> Self {
        Self { model, completion }
    }
}

impl<C: StructuredCompletion> GateModel for FastGateModel<C> {
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
        // Each prompt is read once, so none is written to the prompt cache.
        let call = StructuredCall {
            model: self.model.clone(),
            instructions: question.instructions,
            prompt: question.prompt.clone(),
            schema,
            cache: PromptCache::Skip,
            usage: usage_context(owner, run_id),
        };
        let value = self.completion.complete(call).await?;
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
