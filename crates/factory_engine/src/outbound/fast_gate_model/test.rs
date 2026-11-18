use std::sync::{Arc, Mutex};

use agent::PromptCache;
use ai_usage::{AiFeature, UsageAmount};
use factory_records::domain::models::{Outcome, RunId};
use macro_user_id::user_id::MacroUserIdStr;
use serde_json::json;
use uuid::Uuid;

use super::{FastGateModel, GateModelError, StructuredCall, StructuredCompletion, usage_context};
use crate::domain::ports::{GateModel, ModelQuestion};

/// A Gate model call's usage is the owner's, and names the Run it was
/// about, so it can be told apart and summed per Run.
#[test]
fn a_calls_usage_names_its_run() {
    let owner = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let run_id = RunId::from_uuid(Uuid::from_u128(0x5eed));

    let event = usage_context(&owner, run_id).into_event(
        "anthropic/claude-haiku-5-5".to_string(),
        UsageAmount::Tokens {
            input: 900,
            output: 40,
            cache_read: 0,
            cache_write: 0,
        },
    );

    assert_eq!(event.feature, AiFeature::FactoryGate);
    assert_eq!(event.user, owner);
    assert_eq!(event.entity, Some(Uuid::from_u128(0x5eed)));
}

/// Keeps each call it is asked to make, and answers with a pick.
#[derive(Clone, Default)]
struct KeptCalls(Arc<Mutex<Vec<StructuredCall>>>);

impl StructuredCompletion for KeptCalls {
    async fn complete(&self, call: StructuredCall) -> Result<serde_json::Value, GateModelError> {
        self.0.lock().expect("calls").push(call);
        Ok(json!({ "choice": "approve", "confidence": 0.9, "reason": "Looks right." }))
    }
}

/// A Gate's prompt is read once, so it is sent with no cache write, about
/// the Run it reads, and the model's pick comes back as the Gate's answer.
#[tokio::test]
async fn a_pick_writes_no_prompt_cache_and_names_its_run() {
    let calls = KeptCalls::default();
    let model =
        FastGateModel::with_completion("anthropic/claude-haiku-5-5".to_string(), calls.clone());
    let owner = MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("user");
    let run_id = RunId::from_uuid(Uuid::from_u128(0x5eed));
    let approve = Outcome::try_from("approve".to_string()).expect("valid");

    let pick = model
        .pick(
            &owner,
            run_id,
            &ModelQuestion {
                instructions: "Pick an answer.",
                prompt: "{}".to_string(),
                choices: vec![approve.clone()],
            },
        )
        .await
        .expect("pick");

    assert_eq!(pick.choice, Some(approve));
    let calls = calls.0.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].cache, PromptCache::Skip);
    assert_eq!(calls[0].usage.entity, Some(Uuid::from_u128(0x5eed)));
}
