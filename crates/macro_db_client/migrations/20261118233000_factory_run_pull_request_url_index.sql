-- no-transaction

-- The Reviews page looks up the factory's Runs by the pull requests they
-- opened (crates/factory_records, `pull_request_runs`), in lower case, as
-- GitHub names owners and repositories in any case. Built concurrently, so
-- Runs are kept and read while it builds.
--
-- Not partial: the planner keeps no statistics for the expression of a
-- partial index, so it misjudged the lookup's rows and read the whole table.
--
-- An interrupted build leaves an INVALID index, which IF NOT EXISTS would
-- then skip while the migration is recorded as applied. Before running this
-- migration again after one, drop it:
--   DROP INDEX CONCURRENTLY IF EXISTS factory_run_pull_request_url_idx;
CREATE INDEX CONCURRENTLY IF NOT EXISTS factory_run_pull_request_url_idx
    ON factory_run (lower(pull_request_url));
