INSERT INTO team (id, name, owner_id) VALUES
    ('00000000-0000-7000-8000-000000000201', 'Release', 'macro|julia@example.com');

INSERT INTO team_user (team_id, user_id, team_role) VALUES
    ('00000000-0000-7000-8000-000000000201', 'macro|julia@example.com', 'owner'),
    ('00000000-0000-7000-8000-000000000201', 'macro|teo@example.com', 'member');

-- An agent's account, an admin of the team.
INSERT INTO macro_user (id, username, email, stripe_customer_id) VALUES
    ('00000000-0000-7000-8000-000000000003', 'robot@example.com', 'robot@example.com', 'stripe_robot');

INSERT INTO "User" (id, email, macro_user_id, "isAgent") VALUES
    ('macro|robot@example.com', 'robot@example.com', '00000000-0000-7000-8000-000000000003', true);

INSERT INTO team_user (team_id, user_id, team_role) VALUES
    ('00000000-0000-7000-8000-000000000201', 'macro|robot@example.com', 'admin');
