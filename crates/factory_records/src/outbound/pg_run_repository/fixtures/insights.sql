-- Julia's Runs in November 2026, one of hers in October, and one of Teo's.
INSERT INTO factory_run (id, user_id, flow, request, created_at, outcome, ended_at) VALUES
    -- A plan sent back once, then approved and written.
    ('00000000-0000-7000-8000-000000000201', 'macro|julia@example.com', 'plan',
     E'  Plan the export\nWith the CSV columns.', '2026-11-10 09:00Z', 'success', '2026-11-10 09:30Z'),
    -- A plan whose work failed.
    ('00000000-0000-7000-8000-000000000202', 'macro|julia@example.com', 'plan',
     'Plan the import', '2026-11-11 10:00Z', 'failed', '2026-11-11 10:10Z'),
    -- A reply still going.
    ('00000000-0000-7000-8000-000000000203', 'macro|julia@example.com', 'reply',
     'Summarise the thread', '2026-11-12 08:00Z', NULL, NULL),
    -- A plan approved, then cancelled while it was written.
    ('00000000-0000-7000-8000-000000000204', 'macro|julia@example.com', 'plan',
     'Plan the dark mode', '2026-11-12 12:00Z', 'cancelled', '2026-11-12 12:20Z'),
    -- A Run recorded before the Flow was kept.
    ('00000000-0000-7000-8000-000000000205', 'macro|julia@example.com', NULL,
     'Say hello', '2026-11-13 07:00Z', 'success', '2026-11-13 07:01Z'),
    ('00000000-0000-7000-8000-000000000206', 'macro|teo@example.com', 'plan',
     'Plan the billing page', '2026-11-11 11:00Z', 'success', '2026-11-11 11:30Z'),
    ('00000000-0000-7000-8000-000000000207', 'macro|julia@example.com', 'plan',
     'Plan the last release', '2026-10-01 09:00Z', 'failed', '2026-10-01 09:05Z');

INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at, session_id, action_id, offered) VALUES
    ('00000000-0000-7000-8000-000000000201', 1, 'plan', 1, '2026-11-10 09:00Z', 'done', 'A plan', '2026-11-10 09:05Z', NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000201', 2, 'plan', 2, '2026-11-10 09:15Z', 'done', 'A plan', '2026-11-10 09:18Z', NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000201', 3, 'write', 1, '2026-11-10 09:20Z', 'done', 'Written', '2026-11-10 09:30Z',
     '00000000-0000-7000-8000-000000000301', '00000000-0000-7000-8000-000000000302', '{done,unchanged}'),
    ('00000000-0000-7000-8000-000000000202', 1, 'plan', 1, '2026-11-11 10:00Z', 'failed', 'It failed', '2026-11-11 10:10Z', NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000203', 1, 'reply', 1, '2026-11-12 08:00Z', NULL, NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000204', 1, 'plan', 1, '2026-11-12 12:00Z', 'done', 'A plan', '2026-11-12 12:05Z', NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000204', 2, 'write', 1, '2026-11-12 12:06Z', 'cancelled', 'The run was cancelled.', '2026-11-12 12:20Z', NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000206', 1, 'plan', 1, '2026-11-11 11:00Z', 'failed', 'It failed', '2026-11-11 11:10Z', NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000207', 1, 'plan', 1, '2026-10-01 09:00Z', 'failed', 'It failed', '2026-10-01 09:05Z', NULL, NULL, NULL);

INSERT INTO factory_gate (run_id, seq, question, answers, opened_at, answer, user_id, answered_at) VALUES
    ('00000000-0000-7000-8000-000000000201', 1, 'Approve this plan?', '{approve,revise}', '2026-11-10 09:05Z', 'revise', 'macro|julia@example.com', '2026-11-10 09:15Z'),
    ('00000000-0000-7000-8000-000000000201', 2, 'Approve this plan?', '{approve,revise}', '2026-11-10 09:18Z', 'approve', 'macro|julia@example.com', '2026-11-10 09:20Z'),
    ('00000000-0000-7000-8000-000000000204', 1, 'Approve this plan?', '{approve,revise}', '2026-11-12 12:05Z', 'approve', 'macro|julia@example.com', '2026-11-12 12:06Z');
