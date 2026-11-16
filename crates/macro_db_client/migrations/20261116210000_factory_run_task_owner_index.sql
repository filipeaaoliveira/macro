-- A task's Runs are listed for the person who started them, newest first:
-- this index finds one person's Runs on one task in that order and stops at
-- the page's end. Its leading column also finds a deleted task document's
-- Runs, which the index it replaces did.
--
-- factory_run is new and holds no rows in any deployed database, so the
-- index is built in this transaction rather than concurrently.
SET LOCAL lock_timeout = '5s';

CREATE INDEX factory_run_task_owner_idx
    ON factory_run (task_id, user_id, created_at DESC, id DESC)
    WHERE task_id IS NOT NULL;

DROP INDEX factory_run_task_id_idx;
