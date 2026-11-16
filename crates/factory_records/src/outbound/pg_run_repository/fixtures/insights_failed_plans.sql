-- Seven of Julia's plans on November 1st, an hour apart, each failed in its
-- first Step; the newest went back to plan, and failed again.
INSERT INTO factory_run (id, user_id, flow, request, created_at, outcome, ended_at)
SELECT ('00000000-0000-7000-8000-0000000005' || lpad(n::text, 2, '0'))::uuid,
       'macro|julia@example.com', 'plan', 'Plan ' || n,
       '2026-11-01 00:00Z'::timestamptz + n * interval '1 hour', 'failed',
       '2026-11-01 00:01Z'::timestamptz + n * interval '1 hour'
FROM generate_series(1, 7) AS n;

INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at)
SELECT id, 1, 'plan', 1, created_at, 'failed', 'It failed', ended_at
FROM factory_run
WHERE user_id = 'macro|julia@example.com' AND request LIKE 'Plan %';

INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at)
VALUES ('00000000-0000-7000-8000-000000000507', 2, 'plan', 2,
        '2026-11-01 07:02Z', 'failed', 'It failed again', '2026-11-01 07:03Z');
