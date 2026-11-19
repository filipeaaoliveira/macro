-- Notices filed under a User entity, for the attention candidate query.
-- Loads after `work_feed`, which creates user-1 and user-2.
-- (times on 2024-06-01, UTC)
--
--   12:00  a factory Run without a task waits on user-1, filed under user-1 -> listed
--   12:01  a notice to user-1 filed under user-2, as a sender's invite is   -> not listed
--   user-2 is sent nothing filed under themself                             -> nothing for user-2

SET session_replication_role = 'replica';

INSERT INTO public.notification ("id", "notification_event_type", "event_item_id", "event_item_type", "service_sender", "created_at", "metadata", "sender_id")
VALUES ('0190b000-0000-7000-8000-000000000070', 'factory_gate_waiting', 'macro|user-1@example.com', 'user', 'test', '2024-06-01 12:00:00', '{}', NULL),
       ('0190b000-0000-7000-8000-000000000071', 'factory_gate_waiting', 'macro|user-2@example.com', 'user', 'test', '2024-06-01 12:01:00', '{}', NULL);

INSERT INTO public.user_notification ("user_id", "notification_id", "created_at", "sent", "seen_at", "deleted_at", "state", "is_important_v0")
VALUES ('macro|user-1@example.com', '0190b000-0000-7000-8000-000000000070', '2024-06-01 12:00:00', FALSE, NULL, NULL, 'unseen', FALSE),
       ('macro|user-1@example.com', '0190b000-0000-7000-8000-000000000071', '2024-06-01 12:01:00', FALSE, NULL, NULL, 'unseen', FALSE);

SET session_replication_role = 'origin';
