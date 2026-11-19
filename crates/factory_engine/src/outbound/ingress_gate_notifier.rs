//! Tell a Run's owner of a waiting Gate through the notification ingress:
//! the queue every Macro producer notifies through, from which the
//! notification service creates the row, delivers it live and pushes it.
//! Once the Gate is answered, its notification is marked done through the
//! notifications' reader, as the owner marking it done would, which also
//! clears its push.
//!
//! The ingress creates the notification only when it reads the message, so
//! a Gate answered (or a Run ended) within moments of its notice can come
//! before the notification exists. When there is none to mark, the notice
//! is withdrawn first ([`NotificationWithdrawal`]): the ingress then never
//! creates or pushes it, and marking it done again catches one created in
//! between. Only a notice closed with no notification to mark leaves a
//! withdrawal row: the ones that lost that race, and the ones never sent.

#[cfg(test)]
mod test;

use std::collections::HashSet;
use std::sync::Arc;

use macro_user_id::user_id::MacroUserIdStr;
use model_entity::EntityType;
use notification::domain::models::request::{
    NotificationStatus, SendNotificationRequestBuilder, UpdateNotificationsRequest,
};
use notification::domain::ports::NotificationWithdrawal;
use notification::domain::service::{NotificationIngress, NotificationReader};
use uuid::Uuid;

use crate::domain::gate_owner::GateNotice;
use crate::domain::ports::GateNotifier;

/// What marks a user's notification done: the notifications' reader, as
/// [`NotificationReader::update_notifications`] does it.
pub trait MarkDone: Send + Sync + 'static {
    /// Mark the notification `notification_id` of `user_id` done, and say
    /// whether there was one to mark.
    fn mark_done(
        &self,
        user_id: &MacroUserIdStr<'static>,
        notification_id: Uuid,
    ) -> impl Future<Output = Result<bool, rootcause::Report>> + Send;
}

impl<T: NotificationReader> MarkDone for T {
    async fn mark_done(
        &self,
        user_id: &MacroUserIdStr<'static>,
        notification_id: Uuid,
    ) -> Result<bool, rootcause::Report> {
        let marked = self
            .update_notifications_and_return::<serde_json::Value>(UpdateNotificationsRequest {
                user_id: user_id.clone(),
                notification_ids: &[notification_id],
                status: NotificationStatus::Done(true),
            })
            .await?;
        Ok(!marked.is_empty())
    }
}

/// [`GateNotifier`] over a [`NotificationIngress`], a [`MarkDone`] (the
/// notifications' reader) that marks notifications done, and a
/// [`NotificationWithdrawal`] that stops a notification not yet created.
pub struct IngressGateNotifier<Ingress, Reader, Withdrawal> {
    ingress: Arc<Ingress>,
    reader: Arc<Reader>,
    withdrawal: Arc<Withdrawal>,
}

impl<Ingress, Reader, Withdrawal> Clone for IngressGateNotifier<Ingress, Reader, Withdrawal> {
    fn clone(&self) -> Self {
        Self {
            ingress: Arc::clone(&self.ingress),
            reader: Arc::clone(&self.reader),
            withdrawal: Arc::clone(&self.withdrawal),
        }
    }
}

impl<Ingress, Reader, Withdrawal> IngressGateNotifier<Ingress, Reader, Withdrawal> {
    /// Notify through `ingress`, mark done through `reader`, and withdraw
    /// a notification not yet created through `withdrawal`.
    pub fn new(ingress: Arc<Ingress>, reader: Arc<Reader>, withdrawal: Arc<Withdrawal>) -> Self {
        Self {
            ingress,
            reader,
            withdrawal,
        }
    }
}

impl<Ingress, Reader, Withdrawal> GateNotifier for IngressGateNotifier<Ingress, Reader, Withdrawal>
where
    Ingress: NotificationIngress,
    Reader: MarkDone,
    Withdrawal: NotificationWithdrawal,
{
    /// The notification is about the Run's task, or about the owner for a
    /// Run started without one (as an inbox's sign-in notice is), has no
    /// sender (the factory is no user, so the owner is never dropped as
    /// their own sender), is
    /// delivered live, and pushed to phones when the notice says so, and
    /// keeps the notice's id, so the ingress makes one notification however
    /// often it is sent. A send that fails is logged and dropped: the Gate
    /// waits all the same.
    async fn notify(&self, notice: GateNotice) {
        let run_id = notice.metadata.run_id;
        let seq = notice.metadata.seq;
        let push = notice.push;
        let notification_entity = match notice.task_id {
            Some(task_id) => EntityType::Document.with_entity_string(task_id.to_string()),
            None => EntityType::User.with_entity_string(notice.owner.to_string()),
        };
        let request = SendNotificationRequestBuilder {
            notification_entity,
            secondary_notification_entity: None,
            notification: notice.metadata,
            sender_id: None,
            recipient_ids: HashSet::from([notice.owner]),
        }
        .into_request_with_id(notice.notification_id)
        .with_conn_gateway();
        let sent = if push {
            self.ingress.send_notification(request.with_apns()).await
        } else {
            self.ingress.send_notification(request).await
        };
        if let Err(error) = sent {
            tracing::warn!(?error, %run_id, seq, "a gate's notification was not sent");
        }
    }

    /// A notification there is to mark is marked done. One that is not
    /// there yet (still on the ingress queue, or never sent) is withdrawn,
    /// so the ingress never creates or pushes it, then marked done again,
    /// in case the ingress created it in between.
    async fn answered(&self, owner: &MacroUserIdStr<'static>, notification_id: Uuid) {
        match self.reader.mark_done(owner, notification_id).await {
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(?error, %notification_id, "an answered gate's notification was not marked done");
            }
        }
        if let Err(error) = self.withdrawal.withdraw(notification_id).await {
            tracing::warn!(?error, %notification_id, "an answered gate's notification was not withdrawn");
            return;
        }
        if let Err(error) = self.reader.mark_done(owner, notification_id).await {
            tracing::warn!(?error, %notification_id, "an answered gate's notification was not marked done");
        }
    }
}
