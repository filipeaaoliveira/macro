-- A Benchmark suite keeps how many trials it plans to start
-- (crates/factory_records): its trials are recorded one by one after the
-- suite, so a person's other starts count the planned trials of a suite
-- still starting, not only the trials recorded so far.
--
-- Adding a column with a constant default rewrites no row; the check scans
-- factory_benchmark_suite under its ACCESS EXCLUSIVE lock, which a small
-- table holds briefly. Give up rather than queue behind a long transaction
-- on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_benchmark_suite
    ADD COLUMN planned_trials integer NOT NULL DEFAULT 0 CHECK (planned_trials >= 0);
