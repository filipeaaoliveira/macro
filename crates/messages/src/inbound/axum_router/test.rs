use macro_authorization::{CredentialOrigin, MacroAuthorization, MacroUserAuthentication};
use macro_user_id::user_id::MacroUserIdStr;
use model_user::UserContext;

use super::post_notification_policy;
use crate::domain::models::PostMessageNotificationPolicy;

fn person() -> MacroUserAuthentication {
    MacroUserAuthentication {
        origin: CredentialOrigin::Session,
        agent_id: None,
        agent_via: None,
        routine_id: None,
        macro_user_id: MacroUserIdStr::try_from("macro|julia@example.com".to_string())
            .expect("valid"),
        user_context: UserContext {
            user_id: "macro|julia@example.com".to_string(),
            fusion_user_id: "fusion-user-id".to_string(),
            permissions: None,
            organization_id: None,
        },
    }
}

/// An internal caller acting for a person chooses how its post notifies;
/// a person's own post notifies as usual, whatever it asks.
#[test]
fn only_an_internal_caller_chooses_how_a_post_notifies() {
    let internal = MacroAuthorization::Internal(Some(person()));
    assert_eq!(
        post_notification_policy(&internal, Some(PostMessageNotificationPolicy::Silent)),
        PostMessageNotificationPolicy::Silent
    );
    assert_eq!(
        post_notification_policy(&internal, None),
        PostMessageNotificationPolicy::Default
    );
    let own = MacroAuthorization::User(person());
    assert_eq!(
        post_notification_policy(&own, Some(PostMessageNotificationPolicy::Silent)),
        PostMessageNotificationPolicy::Default
    );
}
