-- Two of Julia's agents: Writer, bound to her Machine `m4`, and Cloud, on an
-- in-memory runtime, bound to no Machine. Needs `people`.
INSERT INTO harnesses (id, name, owner_user_id, created_by) VALUES
    ('00000000-0000-7000-8000-0000000000a1', 'm4', 'macro|julia@example.com', 'macro|julia@example.com'),
    ('00000000-0000-7000-8000-0000000000a2', 'm3', 'macro|julia@example.com', 'macro|julia@example.com');

INSERT INTO bots (id, kind, owner_user_id, name, handle, has_agent) VALUES
    ('00000000-0000-7000-8000-0000000000b1', 'owned', 'macro|julia@example.com', 'Writer', 'writer', true),
    ('00000000-0000-7000-8000-0000000000b2', 'owned', 'macro|julia@example.com', 'Cloud', 'cloud', true);

INSERT INTO agent_configs
    (bot_id, instructions, harness, default_model, channel_scope, is_coding, harness_id)
VALUES
    ('00000000-0000-7000-8000-0000000000b1', '', 'macrod', 'claude-opus-4-5', 'all', true,
     '00000000-0000-7000-8000-0000000000a1'),
    ('00000000-0000-7000-8000-0000000000b2', '', 'in-memory', 'claude-sonnet-4-5', 'all', true, NULL);
