INSERT INTO macro_user (id, username, email, stripe_customer_id) VALUES
    ('00000000-0000-7000-8000-000000000001', 'julia@example.com', 'julia@example.com', 'stripe_julia'),
    ('00000000-0000-7000-8000-000000000002', 'teo@example.com', 'teo@example.com', 'stripe_teo');

INSERT INTO "User" (id, email, macro_user_id) VALUES
    ('macro|julia@example.com', 'julia@example.com', '00000000-0000-7000-8000-000000000001'),
    ('macro|teo@example.com', 'teo@example.com', '00000000-0000-7000-8000-000000000002');
