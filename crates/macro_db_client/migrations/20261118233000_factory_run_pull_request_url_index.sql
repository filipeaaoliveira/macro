-- no-transaction

-- The Reviews page looks up the factory's Runs by the pull requests they
-- opened (crates/factory_records, `pull_request_runs`), in lower case, as
-- GitHub names owners and repositories in any case. Built concurrently, so
-- Runs are kept and read while it builds.
CREATE INDEX CONCURRENTLY IF NOT EXISTS factory_run_pull_request_url_idx
    ON factory_run (lower(pull_request_url))
    WHERE pull_request_url IS NOT NULL;
