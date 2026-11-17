-- The id a Main Hold's task is filed with (crates/factory_records), chosen
-- when the filing is claimed and kept until the hold ends: a filing whose
-- answer was lost is filed again under the same id, so document storage
-- finds the task it made instead of making a second one.
--
-- A nullable column with no default rewrites no rows; the check scans
-- factory_main_hold, a small table, under its ACCESS EXCLUSIVE lock. Give
-- up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_main_hold
    ADD COLUMN task_key uuid,
    ADD CONSTRAINT factory_main_hold_task_key CHECK (task_key IS NULL OR held_since IS NOT NULL);
