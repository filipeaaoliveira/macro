INSERT INTO team (id, name, owner_id) VALUES
    ('00000000-0000-7000-8000-000000000201', 'Release', 'macro|julia@example.com');

INSERT INTO team_user (team_id, user_id, team_role) VALUES
    ('00000000-0000-7000-8000-000000000201', 'macro|julia@example.com', 'owner'),
    ('00000000-0000-7000-8000-000000000201', 'macro|teo@example.com', 'member');
