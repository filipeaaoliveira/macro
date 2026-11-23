-- no-transaction

-- A `ci` Step that may rerun a flaky red reads the CI waits of the last 30
-- days on its repository (crates/factory_records, `ci_flakes`), by when
-- each was kept. Built concurrently, so waits are kept and read while it
-- builds.
--
-- An interrupted build leaves an INVALID index, which IF NOT EXISTS would
-- then skip while the migration is recorded as applied. Before running this
-- migration again after one, drop it:
--   DROP INDEX CONCURRENTLY IF EXISTS factory_ci_wait_recorded_at_idx;
CREATE INDEX CONCURRENTLY IF NOT EXISTS factory_ci_wait_recorded_at_idx
    ON factory_ci_wait (recorded_at);
