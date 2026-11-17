-- A Project of Julia's: an initiative.
INSERT INTO "SharePermission" (id) VALUES ('factory-project-share');

INSERT INTO initiative (id, name, owner_user_id, share_permission_id) VALUES
    ('00000000-0000-7000-8000-000000000301', 'Checkout', 'macro|julia@example.com', 'factory-project-share');
