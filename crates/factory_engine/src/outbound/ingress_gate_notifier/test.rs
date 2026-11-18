use std::sync::Mutex;

use macro_user_id::user_id::MacroUserIdStr;
use model_notifications::FactoryGateWaitingMetadata;
use notification::domain::models::{Notification, NotificationResult, SendNotificationRequest};
use notification::domain::service::{NotificationIngress, SendNotificationError};
use rootcause::Report;
use serde::Serialize;
use uuid::Uuid;

use super::IngressGateNotifier;
use crate::domain::gate_owner::GateNotice;
use crate::domain::ports::GateNotifier;

/// Keeps every request as JSON, as the queue would carry it.
#[derive(Default)]
struct RecordingIngress {
    sent: Mutex<Vec<serde_json::Value>>,
    fail: bool,
}

impl NotificationIngress for RecordingIngress {
    async fn send_notification<
        'a,
        T: Notification + Clone + 'static,
        U: Serialize + Send + Sync + 'static,
    >(
        &'a self,
        req: SendNotificationRequest<'a, T, U>,
    ) -> Result<Option<NotificationResult<'a>>, Report<SendNotificationError>> {
        if self.fail {
            return Err(rootcause::report!("queue down").context(SendNotificationError::Other));
        }
        self.sent
            .lock()
            .expect("sent")
            .push(serde_json::to_value(&req).expect("requests serialize"));
        Ok(None)
    }
}

fn notice() -> GateNotice {
    GateNotice {
        notification_id: Uuid::from_u128(0x1d),
        owner: MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid"),
        task_id: Uuid::from_u128(0x7a5c),
        metadata: FactoryGateWaitingMetadata {
            run_id: Uuid::from_u128(0x2a),
            seq: 3,
            task_id: Uuid::from_u128(0x7a5c).to_string(),
            request: "Fix the login bug".to_string(),
            question: "Approve the plan?".to_string(),
            asker: None,
        },
    }
}

/// The owner is told about the Run's task, under the notice's id, with
/// no sender, live and by push.
#[tokio::test]
async fn the_owner_is_notified_about_the_run_s_task() {
    let ingress = std::sync::Arc::new(RecordingIngress::default());
    IngressGateNotifier::new(ingress.clone())
        .notify(notice())
        .await;

    let sent = ingress.sent.lock().expect("sent");
    assert_eq!(sent.len(), 1);
    let request = sent[0].to_string();
    assert!(request.contains("factory_gate_waiting"), "{request}");
    assert!(request.contains("macro|julia@example.com"), "{request}");
    assert!(
        request.contains(&Uuid::from_u128(0x1d).to_string()),
        "{request}"
    );
    assert!(
        request.contains(&Uuid::from_u128(0x7a5c).to_string()),
        "{request}"
    );
    assert!(request.contains("\"document\""), "{request}");
}

/// A send that fails is dropped: the Gate waits all the same.
#[tokio::test]
async fn a_notice_that_cannot_be_sent_is_dropped() {
    let ingress = std::sync::Arc::new(RecordingIngress {
        fail: true,
        ..RecordingIngress::default()
    });
    IngressGateNotifier::new(ingress.clone())
        .notify(notice())
        .await;
    assert!(ingress.sent.lock().expect("sent").is_empty());
}
