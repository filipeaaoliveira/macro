-- Julia's plan session made two model calls and her write session one; Teo's
-- session one. A call recorded with a session only as its entity, and one
-- with no session, are not a session's usage. The plan session's calls
-- are priced; the write session's call has no price.
INSERT INTO ai_usage (id, feature, user_id, model, input_tokens, output_tokens,
                      cache_read_input_tokens, cache_write_input_tokens,
                      agent_session_id, entity, total)
VALUES
    ('00000000-0000-7000-8000-000000000401', 'agent_session', 'macro|julia@example.com', 'm', 100, 20, 1000, 50, '00000000-0000-7000-8000-000000000301', NULL, 0.5),
    ('00000000-0000-7000-8000-000000000402', 'agent_session', 'macro|julia@example.com', 'm', 30, 5, 400, 0, '00000000-0000-7000-8000-000000000301', NULL, 0.25),
    ('00000000-0000-7000-8000-000000000403', 'agent_session', 'macro|julia@example.com', 'm', 7, 3, 0, 0, '00000000-0000-7000-8000-000000000302', NULL, NULL),
    ('00000000-0000-7000-8000-000000000404', 'agent_session', 'macro|teo@example.com', 'm', 9, 9, 9, 9, '00000000-0000-7000-8000-000000000303', NULL, 1),
    ('00000000-0000-7000-8000-000000000405', 'agent_session', 'macro|julia@example.com', 'm', 8, 8, 8, 8, NULL, '00000000-0000-7000-8000-000000000304', 1),
    ('00000000-0000-7000-8000-000000000406', 'chat', 'macro|julia@example.com', 'm', 5, 5, 5, 5, NULL, NULL, 1);
