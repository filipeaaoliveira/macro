INSERT INTO macro_user (id, username, email, stripe_customer_id) VALUES
    ('00000000-0000-7000-8000-000000000001', 'julia@example.com', 'julia@example.com', 'stripe_julia');

INSERT INTO "User" (id, email, macro_user_id) VALUES
    ('macro|julia@example.com', 'julia@example.com', '00000000-0000-7000-8000-000000000001');
