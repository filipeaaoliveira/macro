use ai_usage::{AiFeature, UsageAmount};
use factory_records::domain::models::RunId;
use macro_user_id::user_id::MacroUserIdStr;
use uuid::Uuid;

use super::usage_context;

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
