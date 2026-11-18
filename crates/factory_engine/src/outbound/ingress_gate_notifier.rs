//! Tell a Run's owner of a waiting Gate through the notification ingress:
//! the queue every Macro producer notifies through, from which the
//! notification service creates the row, delivers it live and pushes it.
//! Once the Gate is answered, its notification is marked done through the
//! notifications' reader, as the owner marking it done would, which also
//! clears its push.

#[cfg(test)]
mod test;

use std::collections::HashSet;
use std::sync::Arc;

use macro_user_id::user_id::MacroUserIdStr;
use model_entity::EntityType;
use notification::domain::models::request::{
    NotificationStatus, SendNotificationRequestBuilder, UpdateNotificationsRequest,
};
use notification::domain::service::{NotificationIngress, NotificationReader};
use uuid::Uuid;

use crate::domain::gate_owner::GateNotice;
use crate::domain::ports::GateNotifier;

/// What marks a user's notification done: the notifications' reader, as
/// [`NotificationReader::update_notifications`] does it.
pub trait MarkDone: Send + Sync + 'static {
    /// Mark the notification `notification_id` of `user_id` done.
    fn mark_done(
        &self,
        user_id: &MacroUserIdStr<'static>,
        notification_id: Uuid,
    ) -> impl Future<Output = Result<(), rootcause::Report>> + Send;
}

impl<T: NotificationReader> MarkDone for T {
    async fn mark_done(
        &self,
        user_id: &MacroUserIdStr<'static>,
        notification_id: Uuid,
    ) -> Result<(), rootcause::Report> {
        self.update_notifications(UpdateNotificationsRequest {
            user_id: user_id.clone(),
            notification_ids: &[notification_id],
            status: NotificationStatus::Done(true),
        })
        .await
    }
}

/// [`GateNotifier`] over a [`NotificationIngress`], and a [`MarkDone`]
/// (the notifications' reader) that marks notifications done.
pub struct IngressGateNotifier<Ingress, Reader> {
    ingress: Arc<Ingress>,
    reader: Arc<Reader>,
}

impl<Ingress, Reader> Clone for IngressGateNotifier<Ingress, Reader> {
    fn clone(&self) -> Self {
        Self {
            ingress: Arc::clone(&self.ingress),
            reader: Arc::clone(&self.reader),
        }
    }
}

impl<Ingress, Reader> IngressGateNotifier<Ingress, Reader> {
    /// Notify through `ingress`, and mark done through `reader`.
    pub fn new(ingress: Arc<Ingress>, reader: Arc<Reader>) -> Self {
        Self { ingress, reader }
    }
}

impl<Ingress, Reader> GateNotifier for IngressGateNotifier<Ingress, Reader>
where
    Ingress: NotificationIngress,
    Reader: MarkDone,
{
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

    async fn answered(&self, owner: &MacroUserIdStr<'static>, notification_id: Uuid) {
        if let Err(error) = self.reader.mark_done(owner, notification_id).await {
            tracing::warn!(?error, %notification_id, "an answered gate's notification was not marked done");
        }
    }
}
