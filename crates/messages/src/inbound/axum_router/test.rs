use macro_authorization::MacroUserAuthentication;
use macro_user_id::user_id::MacroUserIdStr;
use model_user::UserContext;

use super::*;

fn path(parent_type: &str, parent_id: &str) -> ParentPath {
    ParentPath {
        parent_type: parent_type.to_owned(),
        parent_id: parent_id.to_owned(),
        id: None,
        legacy_id: None,
    }
}

fn post(policy: &str) -> PostMessage {
    serde_json::from_value(serde_json::json!({
        "content": "nightly sync finished",
        "notification_policy": policy,
    }))
    .expect("post body")
}

#[test]
fn a_channel_message_may_be_silent() {
    let channel = path("channel", &Uuid::now_v7().to_string());

    assert!(ensure_policy_fits_parent(&channel, &post("Silent")).is_ok());
    assert!(ensure_policy_fits_parent(&channel, &post("MentionsOnly")).is_ok());
}

#[test]
fn other_parents_take_only_the_default_policy() {
    let document = path("document", &Uuid::now_v7().to_string());

    assert!(ensure_policy_fits_parent(&document, &post("Default")).is_ok());
    let refused = ensure_policy_fits_parent(&document, &post("Silent"))
        .expect_err("a silent document comment is refused");
    assert!(matches!(refused.0, MessageError::Invalid(_)));
}

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
/// a person's own post keeps what its body chose, whatever the query asks.
#[test]
fn only_an_internal_caller_chooses_how_a_post_notifies() {
    let internal = MacroAuthorization::Internal(Some(person()));
    assert_eq!(
        post_notification_policy(
            &internal,
            Some(PostMessageNotificationPolicy::Silent),
            PostMessageNotificationPolicy::Default,
        ),
        PostMessageNotificationPolicy::Silent
    );
    assert_eq!(
        post_notification_policy(&internal, None, PostMessageNotificationPolicy::Default),
        PostMessageNotificationPolicy::Default
    );
    let own = MacroAuthorization::User(person());
    assert_eq!(
        post_notification_policy(
            &own,
            Some(PostMessageNotificationPolicy::Silent),
            PostMessageNotificationPolicy::Default,
        ),
        PostMessageNotificationPolicy::Default
    );
    assert_eq!(
        post_notification_policy(
            &own,
            Some(PostMessageNotificationPolicy::Default),
            PostMessageNotificationPolicy::Silent,
        ),
        PostMessageNotificationPolicy::Silent,
        "a person's own channel post keeps the policy its body chose"
    );
}
