-- Julia's code Runs in November 2026 on two repositories, and one of Teo's,
-- with their CI waits, review rounds, Blockers, Gates and steering.
INSERT INTO factory_run (id, user_id, flow, request, repo_url, base_branch, pull_request_url, created_at, outcome, ended_at) VALUES
    -- Merged, its merge Gate answered by a Standing Rule.
    ('00000000-0000-7000-8000-000000000401', 'macro|julia@example.com', 'code',
     'Collapse repeated dashes', 'https://github.com/macro/app', 'main',
     'https://github.com/macro/app/pull/1', '2026-11-10 09:00Z', 'success', '2026-11-10 11:00Z'),
    -- Merged, its merge Gate answered by Julia.
    ('00000000-0000-7000-8000-000000000402', 'macro|julia@example.com', 'code',
     'Trim trailing spaces', 'https://github.com/macro/app', 'main',
     'https://github.com/macro/app/pull/2', '2026-11-11 09:00Z', 'success', '2026-11-11 10:00Z'),
    -- Left to Julia after she steered its CI fix.
    ('00000000-0000-7000-8000-000000000403', 'macro|julia@example.com', 'code',
     'Upgrade the parser', 'https://github.com/macro/app', 'main',
     'https://github.com/macro/app/pull/3', '2026-11-12 09:00Z', 'manual', '2026-11-12 10:00Z'),
    -- Merged on another repository, its merge Gate answered by her
    -- Orchestrator.
    ('00000000-0000-7000-8000-000000000404', 'macro|julia@example.com', 'code',
     'Fix the footer', 'https://github.com/macro/site', 'main',
     'https://github.com/macro/site/pull/4', '2026-11-13 09:00Z', 'success', '2026-11-13 10:00Z'),
    ('00000000-0000-7000-8000-000000000405', 'macro|teo@example.com', 'code',
     'Teo''s change', 'https://github.com/macro/app', 'main',
     'https://github.com/macro/app/pull/5', '2026-11-13 09:00Z', 'success', '2026-11-13 10:00Z');

INSERT INTO factory_step (run_id, seq, entry, visit, started_at, outcome, output, ended_at, session_id, action_id, offered, role, agent) VALUES
    ('00000000-0000-7000-8000-000000000401', 1, 'write', 1, '2026-11-10 09:00Z', 'done', 'Written', '2026-11-10 09:10Z',
     '00000000-0000-7000-8000-000000000501', '00000000-0000-7000-8000-000000000601', '{done}', 'writer', 'primary'),
    ('00000000-0000-7000-8000-000000000401', 2, 'review', 1, '2026-11-10 09:10Z', 'changes', 'Found', '2026-11-10 09:20Z',
     '00000000-0000-7000-8000-000000000502', '00000000-0000-7000-8000-000000000602', '{pass,changes}', 'correctness-reviewer', 'primary'),
    ('00000000-0000-7000-8000-000000000401', 3, 'review', 1, '2026-11-10 09:10Z', 'pass', 'Clean', '2026-11-10 09:20Z',
     '00000000-0000-7000-8000-000000000503', '00000000-0000-7000-8000-000000000603', '{pass,changes}', 'scope-reviewer', 'backup'),
    ('00000000-0000-7000-8000-000000000401', 4, 'fix', 1, '2026-11-10 09:20Z', 'done', 'Fixed', '2026-11-10 09:30Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000401', 5, 'review', 2, '2026-11-10 09:30Z', 'changes', 'Found again', '2026-11-10 09:40Z',
     '00000000-0000-7000-8000-000000000504', '00000000-0000-7000-8000-000000000604', '{pass,changes}', 'correctness-reviewer', 'primary'),
    ('00000000-0000-7000-8000-000000000401', 6, 'ci', 1, '2026-11-10 10:00Z', 'red', 'Red', '2026-11-10 10:20Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000401', 7, 'ci', 2, '2026-11-10 10:30Z', 'green', 'Green', '2026-11-10 10:50Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000401', 8, 'merge-gate', 1, '2026-11-10 10:50Z', 'done', 'Ready', '2026-11-10 10:51Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000402', 1, 'ci', 1, '2026-11-11 09:10Z', 'green', 'Green', '2026-11-11 09:40Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000402', 2, 'merge-gate', 1, '2026-11-11 09:40Z', 'done', 'Ready', '2026-11-11 09:41Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000403', 1, 'ci', 1, '2026-11-12 09:10Z', 'red', 'Red', '2026-11-12 09:30Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000403', 2, 'ci-fix', 1, '2026-11-12 09:30Z', 'done', 'Fixed', '2026-11-12 09:50Z',
     '00000000-0000-7000-8000-000000000505', '00000000-0000-7000-8000-000000000605', '{done}', 'fixer', 'primary'),
    ('00000000-0000-7000-8000-000000000404', 1, 'merge-gate', 1, '2026-11-13 09:40Z', 'done', 'Ready', '2026-11-13 09:41Z', NULL, NULL, NULL, NULL, NULL),
    ('00000000-0000-7000-8000-000000000405', 1, 'ci', 1, '2026-11-13 09:10Z', 'red', 'Red', '2026-11-13 09:30Z', NULL, NULL, NULL, NULL, NULL);

INSERT INTO factory_gate (run_id, seq, question, answers, opened_at, answer, user_id, rule, agent_session_id, answered_at) VALUES
    ('00000000-0000-7000-8000-000000000401', 8, 'Merge?', '{approve,revise}', '2026-11-10 10:51Z', 'approve', NULL, 'merge_when_green', NULL, '2026-11-10 10:51Z'),
    ('00000000-0000-7000-8000-000000000402', 2, 'Merge?', '{approve,revise}', '2026-11-11 09:41Z', 'approve', 'macro|julia@example.com', NULL, NULL, '2026-11-11 09:50Z'),
    ('00000000-0000-7000-8000-000000000404', 1, 'Merge?', '{approve,revise}', '2026-11-13 09:41Z', 'approve', NULL, NULL,
     '00000000-0000-7000-8000-000000000701', '2026-11-13 09:45Z');

INSERT INTO factory_step_steer (run_id, seq, place, action, note, prompt_id) VALUES
    ('00000000-0000-7000-8000-000000000403', 2, 1, 'note', 'Pin the old version', '00000000-0000-7000-8000-000000000801');

INSERT INTO factory_merge (run_id, repo_url, base_branch, pull_request_number, pull_request_url, merged_at) VALUES
    ('00000000-0000-7000-8000-000000000401', 'https://github.com/macro/app', 'main', 1, 'https://github.com/macro/app/pull/1', '2026-11-10 11:00Z'),
    ('00000000-0000-7000-8000-000000000402', 'https://github.com/macro/app', 'main', 2, 'https://github.com/macro/app/pull/2', '2026-11-11 10:00Z'),
    ('00000000-0000-7000-8000-000000000404', 'https://github.com/macro/site', 'main', 4, 'https://github.com/macro/site/pull/4', '2026-11-13 10:00Z');

INSERT INTO factory_ci_wait (run_id, seq, head_sha, verdict, recorded_at) VALUES
    ('00000000-0000-7000-8000-000000000401', 6, '1111111111111111111111111111111111111111', 'red', '2026-11-10 10:20Z'),
    ('00000000-0000-7000-8000-000000000401', 7, '2222222222222222222222222222222222222222', 'green', '2026-11-10 10:50Z'),
    ('00000000-0000-7000-8000-000000000402', 1, '3333333333333333333333333333333333333333', 'green', '2026-11-11 09:40Z'),
    ('00000000-0000-7000-8000-000000000403', 1, '4444444444444444444444444444444444444444', 'red', '2026-11-12 09:30Z'),
    ('00000000-0000-7000-8000-000000000405', 1, '5555555555555555555555555555555555555555', 'red', '2026-11-13 09:30Z');

INSERT INTO factory_ci_job (run_id, seq, name, result, failed_step) VALUES
    ('00000000-0000-7000-8000-000000000401', 6, 'build', 'setup', 'Set up job'),
    ('00000000-0000-7000-8000-000000000401', 6, 'test', 'failed', 'Run tests'),
    ('00000000-0000-7000-8000-000000000401', 7, 'build', 'passed', NULL),
    ('00000000-0000-7000-8000-000000000401', 7, 'test', 'passed', NULL),
    ('00000000-0000-7000-8000-000000000402', 1, 'build', 'passed', NULL),
    ('00000000-0000-7000-8000-000000000402', 1, 'test', 'cancelled', NULL),
    ('00000000-0000-7000-8000-000000000403', 1, 'test', 'failed', 'Run tests'),
    ('00000000-0000-7000-8000-000000000405', 1, 'test', 'failed', 'Run tests');

INSERT INTO factory_review_round (run_id, seq, round, reviewers, head_sha, diff_base, fix_diff, started_at, outcome, ended_at) VALUES
    ('00000000-0000-7000-8000-000000000401', 2, 1, 2, '1111111111111111111111111111111111111111', 'main', false,
     '2026-11-10 09:10Z', 'changes', '2026-11-10 09:20Z'),
    ('00000000-0000-7000-8000-000000000401', 5, 2, 1, '2222222222222222222222222222222222222222',
     '1111111111111111111111111111111111111111', true, '2026-11-10 09:30Z', 'changes', '2026-11-10 09:40Z');

INSERT INTO factory_reviewer_answer (run_id, seq, round_seq, read_at) VALUES
    ('00000000-0000-7000-8000-000000000401', 2, 2, '2026-11-10 09:20Z'),
    ('00000000-0000-7000-8000-000000000401', 3, 2, '2026-11-10 09:20Z'),
    ('00000000-0000-7000-8000-000000000401', 5, 5, '2026-11-10 09:40Z');

INSERT INTO factory_review_finding (run_id, seq, place, round_seq, path, problem, why, kind) VALUES
    ('00000000-0000-7000-8000-000000000401', 2, 1, 2, 'src/dash.rs', 'The empty string panics', 'It unwraps', 'blocker'),
    ('00000000-0000-7000-8000-000000000401', 2, 2, 2, 'README.md', 'The guide is stale', 'Out of the diff', 'follow_up'),
    ('00000000-0000-7000-8000-000000000401', 5, 1, 5, 'src/dash.rs', 'The empty string panics', 'Still unwraps', 'blocker');
