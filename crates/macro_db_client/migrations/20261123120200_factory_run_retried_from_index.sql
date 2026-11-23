-- no-transaction

-- A Run is retried at most once, so two retries never work on one branch
-- at the same time (crates/factory_records, `retry_run`): a second retry
-- of the same Run breaks this index and keeps nothing. Built concurrently,
-- so Runs are kept and read while it builds.
--
-- An interrupted build leaves an INVALID index, which IF NOT EXISTS would
-- then skip while the migration is recorded as applied. Before running this
-- migration again after one, drop it:
--   DROP INDEX CONCURRENTLY IF EXISTS factory_run_retried_from_idx;
CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS factory_run_retried_from_idx
    ON factory_run (retried_from)
    WHERE retried_from IS NOT NULL;
