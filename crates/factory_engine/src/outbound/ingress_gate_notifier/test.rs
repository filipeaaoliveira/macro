use std::sync::Mutex;

use macro_user_id::user_id::MacroUserIdStr;
use model_notifications::FactoryGateWaitingMetadata;
use notification::domain::models::{Notification, NotificationResult, SendNotificationRequest};
use notification::domain::service::{NotificationIngress, SendNotificationError};
use rootcause::Report;
use serde::Serialize;
use uuid::Uuid;

use super::{IngressGateNotifier, MarkDone};
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

/// Keeps the notifications it marked done; fails when `fail` is set.
#[derive(Default)]
struct RecordingReader {
    done: Mutex<Vec<(String, Uuid)>>,
    fail: bool,
}

impl MarkDone for RecordingReader {
    async fn mark_done(
        &self,
        user_id: &MacroUserIdStr<'static>,
        notification_id: Uuid,
    ) -> Result<(), rootcause::Report> {
        if self.fail {
            return Err(rootcause::report!("notifications down"));
        }
        self.done
            .lock()
            .expect("done")
            .push((user_id.to_string(), notification_id));
        Ok(())
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
        push: true,
    }
}

/// The owner is told about the Run's task, under the notice's id, with
/// no sender, live and by push.
#[tokio::test]
async fn the_owner_is_notified_about_the_run_s_task() {
    let ingress = std::sync::Arc::new(RecordingIngress::default());
    IngressGateNotifier::new(
        ingress.clone(),
        std::sync::Arc::new(RecordingReader::default()),
    )
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

/// A notify-only Gate's notice is delivered live, and not pushed.
#[tokio::test]
async fn a_notice_that_is_not_pushed_is_delivered_live_only() {
    let ingress = std::sync::Arc::new(RecordingIngress::default());
    let notifier = IngressGateNotifier::new(
        ingress.clone(),
        std::sync::Arc::new(RecordingReader::default()),
    );
    notifier.notify(notice()).await;
    notifier
        .notify(GateNotice {
            push: false,
            ..notice()
        })
        .await;

    let sent = ingress.sent.lock().expect("sent");
    assert_eq!(sent.len(), 2);
    assert!(!sent[0]["build_apns"].is_null(), "{}", sent[0]);
    assert!(sent[1]["build_apns"].is_null(), "{}", sent[1]);
    assert_eq!(sent[1]["send_conn_gateway"], true, "{}", sent[1]);
}

/// A send that fails is dropped: the Gate waits all the same.
#[tokio::test]
async fn a_notice_that_cannot_be_sent_is_dropped() {
    let ingress = std::sync::Arc::new(RecordingIngress {
        fail: true,
        ..RecordingIngress::default()
    });
    IngressGateNotifier::new(
        ingress.clone(),
        std::sync::Arc::new(RecordingReader::default()),
    )
    .notify(notice())
    .await;
    assert!(ingress.sent.lock().expect("sent").is_empty());
}

/// An answered Gate's notification is marked done for its owner; one that
/// cannot be marked is left, and nothing fails.
#[tokio::test]
async fn an_answered_gate_s_notification_is_marked_done() {
    let reader = std::sync::Arc::new(RecordingReader::default());
    let notifier = IngressGateNotifier::new(
        std::sync::Arc::new(RecordingIngress::default()),
        reader.clone(),
    );
    notifier
        .answered(&notice().owner, Uuid::from_u128(0x1d))
        .await;
    assert_eq!(
        *reader.done.lock().expect("done"),
        [("macro|julia@example.com".to_string(), Uuid::from_u128(0x1d))]
    );

    let failing = IngressGateNotifier::new(
        std::sync::Arc::new(RecordingIngress::default()),
        std::sync::Arc::new(RecordingReader {
            fail: true,
            ..RecordingReader::default()
        }),
    );
    failing
        .answered(&notice().owner, Uuid::from_u128(0x1d))
        .await;
}
