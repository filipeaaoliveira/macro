-- no-transaction

-- A `ci` Step that may rerun a flaky red reads the failures other Runs on
-- its repository proved flaky, and the jobs quarantined there
-- (crates/factory_records, `ci_flakes`), by the repository's URL in lower
-- case, as GitHub names owners and repositories in any case. Built
-- concurrently, so Runs are kept and read while it builds.
--
-- An interrupted build leaves an INVALID index, which IF NOT EXISTS would
-- then skip while the migration is recorded as applied. Before running this
-- migration again after one, drop it:
--   DROP INDEX CONCURRENTLY IF EXISTS factory_run_repo_url_idx;
CREATE INDEX CONCURRENTLY IF NOT EXISTS factory_run_repo_url_idx
    ON factory_run (lower(repo_url));
