//! Tell a Run's owner of a waiting Gate through the notification ingress:
//! the queue every Macro producer notifies through, from which the
//! notification service creates the row, delivers it live and pushes it.

#[cfg(test)]
mod test;

use std::collections::HashSet;
use std::sync::Arc;

use model_entity::EntityType;
use notification::domain::models::request::SendNotificationRequestBuilder;
use notification::domain::service::NotificationIngress;

use crate::domain::gate_owner::GateNotice;
use crate::domain::ports::GateNotifier;

/// [`GateNotifier`] over a [`NotificationIngress`].
pub struct IngressGateNotifier<Ingress> {
    ingress: Arc<Ingress>,
}

impl<Ingress> IngressGateNotifier<Ingress> {
    /// Notify through `ingress`.
    pub fn new(ingress: Arc<Ingress>) -> Self {
        Self { ingress }
    }
}

impl<Ingress: NotificationIngress> GateNotifier for IngressGateNotifier<Ingress> {
    /// The notification is about the Run's task, has no sender (the factory
    /// is no user, so the owner is never dropped as their own sender), is
    /// pushed to phones and delivered live, and keeps the notice's id, so
    /// the ingress makes one notification however often it is sent. A send
    /// that fails is logged and dropped: the Gate waits all the same.
    async fn notify(&self, notice: GateNotice) {
        let run_id = notice.metadata.run_id;
        let seq = notice.metadata.seq;
        let request = SendNotificationRequestBuilder {
            notification_entity: EntityType::Document
                .with_entity_string(notice.task_id.to_string()),
            secondary_notification_entity: None,
            notification: notice.metadata,
            sender_id: None,
            recipient_ids: HashSet::from([notice.owner]),
        }
        .into_request_with_id(notice.notification_id)
        .with_apns()
        .with_conn_gateway();
        if let Err(error) = self.ingress.send_notification(request).await {
            tracing::warn!(?error, %run_id, seq, "a waiting gate's notification was not sent");
        }
    }
}
