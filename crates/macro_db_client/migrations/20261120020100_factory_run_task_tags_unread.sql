-- The Runs whose task's tags could not be read as they started
-- (crates/factory_records): their Factory Definition's Agent Rules on a tag
-- were matched against no tags, so the Run API can tell that apart from a
-- task no rule matched. A Run keeps it once, with its Agent Rules.
--
-- The new table references factory_run, so creating it takes a SHARE ROW
-- EXCLUSIVE lock on factory_run until this transaction ends. No existing
-- row is rewritten. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

CREATE TABLE factory_run_task_tags_unread (
    run_id uuid PRIMARY KEY REFERENCES factory_run (id) ON DELETE CASCADE,
    recorded_at timestamptz NOT NULL DEFAULT now()
);
