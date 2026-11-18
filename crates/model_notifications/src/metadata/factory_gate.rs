//! Notification sent to a software factory Run's owner when one of its
//! Gates waits on them: no Standing Rule answered it, or an agent asked
//! them a question; or, flagged `notify_only`, when a notify-only Gate
//! tells them something while the Run goes on.
use super::*;

/// The most characters of the Gate's question, or of the Run's request,
/// a notification carries.
const FACTORY_EXCERPT_MAX_CHARS: usize = 200;

/// A factory Run waits on its owner at a Gate. The notification is about
/// the Run's task, and opens the Run, where the owner answers the Gate.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FactoryGateWaitingMetadata {
    /// The Run that waits.
    pub run_id: Uuid,
    /// The place, among the Run's Steps, of the Step whose Gate waits.
    pub seq: u16,
    /// The task document the Run works on.
    pub task_id: String,
    /// The start of what the Run was asked to do.
    pub request: String,
    /// The start of the Gate's question.
    pub question: String,
    /// For an agent's question, the Role of the agent that asks; absent
    /// for a Gate of the Run's Flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asker: Option<String>,
    /// Whether the Gate only tells the owner, for their information, and
    /// the Run goes on without an answer; absent for a Gate that waits.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notify_only: bool,
}

impl FactoryGateWaitingMetadata {
    /// The start of `text`, on one line, at most
    /// [`FACTORY_EXCERPT_MAX_CHARS`] characters long.
    pub fn excerpt(text: &str) -> String {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.chars().count() <= FACTORY_EXCERPT_MAX_CHARS {
            return flat;
        }
        let mut cut: String = flat.chars().take(FACTORY_EXCERPT_MAX_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

impl Notification for FactoryGateWaitingMetadata {
    const TYPE_NAME: &'static str = "factory_gate_waiting";
}

impl NotificationTitle for FactoryGateWaitingMetadata {
    fn format_title(&self, _sender_id: Option<MacroUserIdStr<'_>>) -> Result<String, Report> {
        if self.notify_only {
            return Ok("For your information: a factory run goes on".to_owned());
        }
        Ok(match &self.asker {
            Some(role) => format!("The {role} asks you a question"),
            None => "A factory run waits on your answer".to_owned(),
        })
    }

    fn format_body(&self, _sender_id: Option<MacroUserIdStr<'_>>) -> Result<String, Report> {
        Ok(format!("{}: {}", self.request, self.question))
    }
}

impl NotificationExtIos for FactoryGateWaitingMetadata {
    type NotifData = PushNotificationData;

    fn collapse_key(&self, _entity: &Entity<'_>) -> NotifCollapseKey {
        // One alert per Gate: a Run asks again only at another Step.
        NotifCollapseKey::new(Self::TYPE_NAME)
            .append(&self.run_id.to_string())
            .append(&self.seq.to_string())
    }

    fn as_apns<'a>(
        &self,
        sender_id: Option<MacroUserIdStr<'a>>,
        _entity: &Entity<'_>,
        notification_id: Uuid,
    ) -> Option<APNSPushNotification<Self::NotifData>> {
        alert_apns(self, sender_id, notification_id, None).ok()
    }
}
