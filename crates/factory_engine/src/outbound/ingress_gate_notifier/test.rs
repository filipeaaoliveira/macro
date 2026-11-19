use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use macro_user_id::user_id::MacroUserIdStr;
use model_notifications::FactoryGateWaitingMetadata;
use notification::domain::models::{Notification, NotificationResult, SendNotificationRequest};
use notification::domain::ports::NotificationWithdrawal;
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

/// Keeps the notifications it marked done, each there to mark; fails when
/// `fail` is set.
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
    ) -> Result<bool, rootcause::Report> {
        if self.fail {
            return Err(rootcause::report!("notifications down"));
        }
        self.done
            .lock()
            .expect("done")
            .push((user_id.to_string(), notification_id));
        Ok(true)
    }
}

/// Keeps the notifications it withdrew.
#[derive(Default)]
struct RecordingWithdrawal {
    withdrawn: Mutex<Vec<Uuid>>,
}

impl NotificationWithdrawal for RecordingWithdrawal {
    async fn withdraw(&self, notification_id: Uuid) -> Result<(), rootcause::Report> {
        self.withdrawn
            .lock()
            .expect("withdrawn")
            .push(notification_id);
        Ok(())
    }
}

/// A notifier over `ingress` and `reader`, withdrawing into `withdrawal`.
fn notifier<I, R>(
    ingress: Arc<I>,
    reader: Arc<R>,
) -> IngressGateNotifier<I, R, RecordingWithdrawal> {
    IngressGateNotifier::new(ingress, reader, Arc::new(RecordingWithdrawal::default()))
}

/// The notification service as the ingress queue feeds it: a sent notice
/// waits on the queue until [`Notifications::deliver`] reads it, which
/// creates and pushes it unless it was withdrawn, as the service's
/// repository does.
#[derive(Default)]
struct Notifications {
    queued: Mutex<Vec<Uuid>>,
    created: Mutex<HashSet<Uuid>>,
    done: Mutex<HashSet<Uuid>>,
    withdrawn: Mutex<HashSet<Uuid>>,
    pushed: Mutex<Vec<Uuid>>,
}

impl Notifications {
    /// Read every queued notice, as the ingress does.
    fn deliver(&self) {
        for id in self.queued.lock().expect("queued").drain(..) {
            if self.withdrawn.lock().expect("withdrawn").contains(&id) {
                continue;
            }
            if self.created.lock().expect("created").insert(id) {
                self.pushed.lock().expect("pushed").push(id);
            }
        }
    }

    /// The notifications created and not marked done.
    fn open(&self) -> Vec<Uuid> {
        let done = self.done.lock().expect("done");
        self.created
            .lock()
            .expect("created")
            .iter()
            .filter(|id| !done.contains(id))
            .copied()
            .collect()
    }
}

impl NotificationIngress for Notifications {
    async fn send_notification<
        'a,
        T: Notification + Clone + 'static,
        U: Serialize + Send + Sync + 'static,
    >(
        &'a self,
        req: SendNotificationRequest<'a, T, U>,
    ) -> Result<Option<NotificationResult<'a>>, Report<SendNotificationError>> {
        let sent = serde_json::to_value(&req).expect("requests serialize");
        let id = sent["uuid_to_write"]
            .as_str()
            .and_then(|id| id.parse().ok())
            .expect("the notice keeps its id");
        self.queued.lock().expect("queued").push(id);
        Ok(None)
    }
}

impl MarkDone for Notifications {
    async fn mark_done(
        &self,
        _user_id: &MacroUserIdStr<'static>,
        notification_id: Uuid,
    ) -> Result<bool, rootcause::Report> {
        let there = self
            .created
            .lock()
            .expect("created")
            .contains(&notification_id);
        if there {
            self.done.lock().expect("done").insert(notification_id);
        }
        Ok(there)
    }
}

impl NotificationWithdrawal for Notifications {
    async fn withdraw(&self, notification_id: Uuid) -> Result<(), rootcause::Report> {
        self.withdrawn
            .lock()
            .expect("withdrawn")
            .insert(notification_id);
        Ok(())
    }
}

fn notice() -> GateNotice {
    GateNotice {
        notification_id: Uuid::from_u128(0x1d),
        owner: MacroUserIdStr::try_from("macro|julia@example.com".to_string()).expect("valid"),
        task_id: Some(Uuid::from_u128(0x7a5c)),
        metadata: FactoryGateWaitingMetadata {
            run_id: Uuid::from_u128(0x2a),
            seq: 3,
            task_id: Uuid::from_u128(0x7a5c).to_string(),
            request: "Fix the login bug".to_string(),
            question: "Approve the plan?".to_string(),
            asker: None,
            notify_only: false,
        },
        push: true,
    }
}

/// The owner is told about the Run's task, under the notice's id, with
/// no sender, live and by push.
#[tokio::test]
async fn the_owner_is_notified_about_the_run_s_task() {
    let ingress = Arc::new(RecordingIngress::default());
    notifier(ingress.clone(), Arc::new(RecordingReader::default()))
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

/// A Run started without a task notifies its owner all the same: the
/// notification is about the owner, still pushed.
#[tokio::test]
async fn the_owner_of_a_run_without_a_task_is_notified_about_themselves() {
    let ingress = Arc::new(RecordingIngress::default());
    let mut taskless = notice();
    taskless.task_id = None;
    taskless.metadata.task_id = String::new();
    notifier(ingress.clone(), Arc::new(RecordingReader::default()))
    .notify(taskless)
    .await;

    let sent = ingress.sent.lock().expect("sent");
    assert_eq!(sent.len(), 1);
    let request = sent[0].to_string();
    assert!(request.contains("factory_gate_waiting"), "{request}");
    assert!(!request.contains("\"document\""), "{request}");
    assert!(request.contains("\"user\""), "{request}");
    assert!(!sent[0]["build_apns"].is_null(), "{request}");
}

/// A notify-only Gate's notice is delivered live, and not pushed.
#[tokio::test]
async fn a_notice_that_is_not_pushed_is_delivered_live_only() {
    let ingress = Arc::new(RecordingIngress::default());
    let sender = notifier(ingress.clone(), Arc::new(RecordingReader::default()));
    sender.notify(notice()).await;
    sender
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
    let ingress = Arc::new(RecordingIngress {
        fail: true,
        ..RecordingIngress::default()
    });
    notifier(ingress.clone(), Arc::new(RecordingReader::default()))
    .notify(notice())
    .await;
    assert!(ingress.sent.lock().expect("sent").is_empty());
}

/// An answered Gate's notification is marked done for its owner; one that
/// cannot be marked is left, and nothing fails.
#[tokio::test]
async fn an_answered_gate_s_notification_is_marked_done() {
    let reader = Arc::new(RecordingReader::default());
    let withdrawal = Arc::new(RecordingWithdrawal::default());
    let marking = IngressGateNotifier::new(
        Arc::new(RecordingIngress::default()),
        reader.clone(),
        withdrawal.clone(),
    );
    marking
        .answered(&notice().owner, Uuid::from_u128(0x1d))
        .await;
    assert_eq!(
        *reader.done.lock().expect("done"),
        [("macro|julia@example.com".to_string(), Uuid::from_u128(0x1d))]
    );
    // One there to mark leaves no withdrawal behind.
    assert!(withdrawal.withdrawn.lock().expect("withdrawn").is_empty());

    let failing = notifier(
        Arc::new(RecordingIngress::default()),
        Arc::new(RecordingReader {
            fail: true,
            ..RecordingReader::default()
        }),
    );
    failing
        .answered(&notice().owner, Uuid::from_u128(0x1d))
        .await;
}

/// A Gate answered before the ingress read its notice: the notice is
/// withdrawn, so the ingress neither creates nor pushes it, and nothing is
/// left open.
#[tokio::test]
async fn a_gate_answered_before_its_notice_is_created_leaves_nothing_open() {
    let notifications = Arc::new(Notifications::default());
    let sender = IngressGateNotifier::new(
        notifications.clone(),
        notifications.clone(),
        notifications.clone(),
    );
    sender.notify(notice()).await;
    sender
        .answered(&notice().owner, notice().notification_id)
        .await;
    notifications.deliver();

    assert!(notifications.open().is_empty(), "{:?}", notifications.open());
    assert!(notifications.pushed.lock().expect("pushed").is_empty());
}

/// A Gate answered once its notice is created marks it done, and
/// withdraws nothing, so a closed notice leaves no row behind.
#[tokio::test]
async fn a_gate_answered_after_its_notice_is_created_marks_it_done() {
    let notifications = Arc::new(Notifications::default());
    let sender = IngressGateNotifier::new(
        notifications.clone(),
        notifications.clone(),
        notifications.clone(),
    );
    sender.notify(notice()).await;
    notifications.deliver();
    sender
        .answered(&notice().owner, notice().notification_id)
        .await;

    assert!(notifications.open().is_empty(), "{:?}", notifications.open());
    assert_eq!(
        *notifications.pushed.lock().expect("pushed"),
        [notice().notification_id]
    );
    assert!(notifications.withdrawn.lock().expect("withdrawn").is_empty());
}
