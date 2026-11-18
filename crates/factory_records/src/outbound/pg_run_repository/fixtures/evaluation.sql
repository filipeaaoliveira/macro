-- Julia's Runs in November 2026: two code Runs on tasks, each in an arm of
-- the writer Experiment, and one Benchmark trial with its suite.
INSERT INTO factory_run (id, user_id, flow, request, created_at, outcome, ended_at) VALUES
    ('00000000-0000-7000-8000-000000000401', 'macro|julia@example.com', 'code',
     'Add the export', '2026-11-10 09:00Z', 'success', '2026-11-10 10:00Z'),
    ('00000000-0000-7000-8000-000000000402', 'macro|julia@example.com', 'code',
     'Add the import', '2026-11-11 09:00Z', 'failed', '2026-11-11 10:00Z'),
    ('00000000-0000-7000-8000-000000000403', 'macro|julia@example.com', 'benchmark',
     'Fix the typo', '2026-11-12 09:00Z', 'success', '2026-11-12 09:30Z');

INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at, session_id, action_id, offered) VALUES
    ('00000000-0000-7000-8000-000000000403', 1, 'plan', 1, '2026-11-12 09:00Z', 'done', 'A plan', '2026-11-12 09:05Z', NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000403', 2, 'write', 1, '2026-11-12 09:06Z', 'done', 'Written', '2026-11-12 09:30Z',
     '00000000-0000-7000-8000-000000000501', '00000000-0000-7000-8000-000000000502', '{done,unchanged,red}');

INSERT INTO factory_run_arm (run_id, experiment, arm, agents) VALUES
    ('00000000-0000-7000-8000-000000000401', 'writer', 'control', '{}'),
    ('00000000-0000-7000-8000-000000000402', 'writer', 'backup', '{"writer": "backup"}');

INSERT INTO factory_benchmark_suite (id, user_id, benchmark, agents, created_at) VALUES
    ('00000000-0000-7000-8000-000000000601', 'macro|julia@example.com', 'core',
     '{"planner": "backup"}', '2026-11-12 08:59Z');

INSERT INTO factory_benchmark_trial (suite_id, task, run_id) VALUES
    ('00000000-0000-7000-8000-000000000601', 'typo', '00000000-0000-7000-8000-000000000403');
