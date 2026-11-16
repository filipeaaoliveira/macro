-- Factory insights (crates/factory_records): keep the Flow each Run walks,
-- and read one person's Runs by when they were created.
--
-- factory_run is new with the factory and holds no deployed rows, so the
-- index is built in this transaction rather than concurrently. Give up
-- rather than queue behind a long transaction on the table.
SET LOCAL lock_timeout = '5s';

-- The built-in Flow the Run walks, by name. Null only for Runs recorded
-- before the Flow was kept.
ALTER TABLE factory_run
    ADD COLUMN flow text CHECK (flow ~ '^[a-z0-9_-]{1,64}$');

-- Insights read one person's Runs created in a time range, newest first, and
-- stop at a cap. Its leading column also finds a deleted user's Runs, which
-- factory_run_user_id_idx did, so that index goes.
CREATE INDEX factory_run_user_created_idx
    ON factory_run (user_id, created_at DESC, id DESC);

DROP INDEX factory_run_user_id_idx;
