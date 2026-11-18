-- A Benchmark trial keeps the commit its write starts from, so every trial
-- of a task replays the same code, and when its Run's branch was deleted
-- once it was scored (crates/factory_records).
--
-- Adding nullable columns rewrites no row; the check scans
-- factory_benchmark_trial under its ACCESS EXCLUSIVE lock, which a small
-- table holds briefly. Give up rather than queue behind a long transaction
-- on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_benchmark_trial
    ADD COLUMN base_commit text CHECK (base_commit ~ '^[0-9a-f]{40}$'),
    ADD COLUMN branch_deleted_at timestamptz;
