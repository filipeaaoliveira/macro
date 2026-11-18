-- A paused factory Step still ends within its time (crates/factory_records):
-- its work records when it ended the Step because it stayed paused past
-- it, so the Step says why it failed. Null for every other Step.
--
-- Adding a nullable column rewrites no row; the check scans factory_step
-- under its ACCESS EXCLUSIVE lock, which a small table holds briefly. Give
-- up rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step
    ADD COLUMN pause_ran_out_at timestamptz,
    ADD CONSTRAINT factory_step_pause_ran_out CHECK (
        pause_ran_out_at IS NULL OR paused_at IS NOT NULL
    );
