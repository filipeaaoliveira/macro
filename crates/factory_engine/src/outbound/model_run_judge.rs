//! The Scorers' model on Macro's model router: one structured completion
//! per judgment, on a small, cheap model, with its usage recorded against
//! the Run's owner as the factory's small model (`AiFeature::FactoryGate`,
//! billed to no one), as the Gate model's is.

use std::sync::Arc;

use agent::Message;
use agent::structured_output::{DynamicSchema, dynamic_structured_completion};
use ai_usage::{AiFeature, UsageContext, UsageRecorder};
use macro_user_id::user_id::MacroUserIdStr;
use serde::Deserialize;
use serde_json::json;

use crate::domain::ports::{JudgeQuestion, JudgeVerdict, RunJudge};

/// The model a Scorer judges on unless told otherwise: Claude Haiku 5.5, by
/// the id Macro's model router knows, as the Gate model.
pub const DEFAULT_SCORER_MODEL: &str = "anthropic/claude-haiku-5-5";

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
