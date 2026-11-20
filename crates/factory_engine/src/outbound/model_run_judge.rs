//! The Scorers' model on Macro's model router: one structured completion
//! per judgment, on a small, cheap model, with its usage recorded against
//! the Run's owner as the factory's small model (`AiFeature::FactoryGate`,
//! billed to no one), as the Gate model's is.

use std::sync::Arc;

use agent::Message;
use agent::structured_output::{DynamicSchema, dynamic_structured_completion};
use ai_usage::{AiFeature, UsageContext, UsageRecorder};
use factory_records::domain::evaluation::MAX_MODEL_BYTES;
use macro_user_id::user_id::MacroUserIdStr;
use serde::Deserialize;
use serde_json::json;

use crate::domain::ports::{JudgeQuestion, JudgeVerdict, RunJudge};

/// The model a Scorer judges on unless told otherwise: Claude Haiku 5.5, by
/// the id Macro's model router knows, as the Gate model.
pub const DEFAULT_SCORER_MODEL: &str = "anthropic/claude-haiku-5-5";

/// Why a Scorers' model id is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("the scorer model id must be 1 to {MAX_MODEL_BYTES} bytes")]
pub struct ScorerModelError;

/// `model`, once it fits the judgments' record of it: 1 to
/// [`MAX_MODEL_BYTES`] bytes, so a judgment is never refused for its model.
pub fn checked_scorer_model(model: String) -> Result<String, ScorerModelError> {
    if model.trim().is_empty() || model.len() > MAX_MODEL_BYTES {
        return Err(ScorerModelError);
    }
    Ok(model)
}

/// Why a judge's call failed.
#[derive(Debug, thiserror::Error)]
pub enum RunJudgeError {
    /// The model could not be reached, or did not answer in the schema.
    #[error("the scorer model's call failed: {0}")]
    Call(String),
}

/// The model's answer, as the schema asks it.
#[derive(Debug, Deserialize)]
struct Judged {
    score: f64,
    reason: String,
}

/// A Scorers' model on Macro's model router.
#[derive(Clone)]
pub struct ModelRunJudge {
    model: String,
    recorder: Arc<dyn UsageRecorder>,
}

impl ModelRunJudge {
    /// The judge on `model`, a model id the router knows, such as
    /// [`DEFAULT_SCORER_MODEL`]; its usage is recorded through `recorder`.
    pub fn new(model: String, recorder: Arc<dyn UsageRecorder>) -> Self {
        Self { model, recorder }
    }
}

impl RunJudge for ModelRunJudge {
    type Err = RunJudgeError;

    fn model(&self) -> &str {
        &self.model
    }

    #[tracing::instrument(err, skip_all, fields(model = %self.model))]
    async fn judge(
        &self,
        owner: &MacroUserIdStr<'static>,
        question: &JudgeQuestion,
    ) -> Result<JudgeVerdict, RunJudgeError> {
        let schema = DynamicSchema {
            name: "RunScore".to_string(),
            description: Some("How well the run does on the dimension, and why.".to_string()),
            schema: json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["score", "reason"],
                "properties": {
                    "score": { "type": "number", "minimum": 0, "maximum": 1 },
                    "reason": { "type": "string" }
                }
            }),
        };
        let value = dynamic_structured_completion(
            self.model.as_str(),
            question.instructions.as_str(),
            vec![Message::user(question.prompt.clone())],
            schema,
            self.recorder.as_ref(),
            UsageContext::new(AiFeature::FactoryGate, owner.clone()),
        )
        .await
        .map_err(|error| RunJudgeError::Call(error.to_string()))?;
        let judged: Judged = serde_json::from_value(value)
            .map_err(|error| RunJudgeError::Call(error.to_string()))?;
        Ok(JudgeVerdict {
            score: judged.score,
            reason: judged.reason,
        })
    }
}

#[cfg(test)]
mod test {
    use super::{DEFAULT_SCORER_MODEL, checked_scorer_model};

    #[test]
    fn a_scorer_model_id_must_fit_its_record() {
        assert!(checked_scorer_model(DEFAULT_SCORER_MODEL.to_string()).is_ok());
        assert!(checked_scorer_model(String::new()).is_err());
        assert!(checked_scorer_model("m".repeat(201)).is_err());
        assert!(checked_scorer_model("m".repeat(200)).is_ok());
    }
}
