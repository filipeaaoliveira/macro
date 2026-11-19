use super::*;
use crate::domain::models::{
    apple::Aps,
    mobile::{MessageAttributes, PushType},
    queue_message::{APNSTargets, UserApnsEndpoints},
};

#[tokio::test]
async fn queued_requests_delivered_out_of_order_skip_all_withdrawn_revisions() {
    let first = Uuid::now_v7();
    let second = Uuid::now_v7();
    let latest = Uuid::now_v7();
    let mut service = create_egress_service(allowing_rate_limiter());
    service
        .repository
        .withdrawn_notifications
        .extend([first, second]);

    for id in [latest, second, first] {
        let recipient = test_user_id("reviewer@example.com");
        let mut notification = create_mock_notif(TestNotification {
            message: "Requested access".to_owned(),
        });
        notification.notification_id = id;
        let websocket = QueueMessage::new_test(
            "entity_access_requested".to_owned(),
            NotificationChannel::ConnGateway(
                ConnGatewayNotification {
                    notif: notification,
                    recipients: vec![recipient.clone()],
                }
                .testing_to_value(),
            ),
        );
        let mobile = QueueMessage::new_test(
            "entity_access_requested".to_owned(),
            NotificationChannel::Ios(Box::new(APNSTargets {
                notif: APNSPushNotification {
                    aps: Aps::default(),
                    push_notification_data: json!({ "notificationId": id }),
                },
                attributes: MessageAttributes {
                    push_type: PushType::Alert,
                    collapse_key: "request-id".to_owned(),
                },
                ios_device_endpoints: HashMap::from([(
                    recipient,
                    UserApnsEndpoints {
                        endpoints: vec!["ios-endpoint".to_owned()],
                        android_endpoints: vec!["android-endpoint".to_owned()],
                        digest_state: None,
                    },
                )]),
            })),
        );
        for message in [websocket, mobile] {
            let results = service.deliver_notification(message).await;
            if id == latest {
                assert!(!results.is_empty());
                assert!(results.iter().all(Result::is_ok));
            } else {
                assert!(
                    results.is_empty(),
                    "withdrawn requests must never alert a reviewer"
                );
            }
        }
    }
}

/// A factory Gate's notice withdrawn once its Gate was answered is not
/// delivered, even when the ingress created it first: its push could
/// otherwise land after the push clear and stay on the phone.
#[tokio::test]
async fn withdrawn_factory_gate_notices_are_not_delivered() {
    let withdrawn = Uuid::now_v7();
    let kept = Uuid::now_v7();
    let mut service = create_egress_service(allowing_rate_limiter());
    service.repository.withdrawn_notifications.insert(withdrawn);

    for id in [withdrawn, kept] {
        let recipient = test_user_id("owner@example.com");
        let mut notification = create_mock_notif(TestNotification {
            message: "A gate waits on you".to_owned(),
        });
        notification.notification_id = id;
        let websocket = QueueMessage::new_test(
            "factory_gate_waiting".to_owned(),
            NotificationChannel::ConnGateway(
                ConnGatewayNotification {
                    notif: notification,
                    recipients: vec![recipient.clone()],
                }
                .testing_to_value(),
            ),
        );
        let mobile = QueueMessage::new_test(
            "factory_gate_waiting".to_owned(),
            NotificationChannel::Ios(Box::new(APNSTargets {
                notif: APNSPushNotification {
                    aps: Aps::default(),
                    push_notification_data: json!({ "notificationId": id }),
                },
                attributes: MessageAttributes {
                    push_type: PushType::Alert,
                    collapse_key: "gate".to_owned(),
                },
                ios_device_endpoints: HashMap::from([(
                    recipient,
                    UserApnsEndpoints {
                        endpoints: vec!["ios-endpoint".to_owned()],
                        android_endpoints: vec![],
                        digest_state: None,
                    },
                )]),
            })),
        );
        for message in [websocket, mobile] {
            let results = service.deliver_notification(message).await;
            if id == kept {
                assert!(!results.is_empty());
                assert!(results.iter().all(Result::is_ok));
            } else {
                assert!(results.is_empty(), "a withdrawn gate notice is delivered");
            }
        }
    }
}
