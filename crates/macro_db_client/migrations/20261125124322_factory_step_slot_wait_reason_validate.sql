-- Validate the Steps' wait reason check added NOT VALID in 20261125115030,
-- in its own transaction, so the lock that migration took is not held
-- through the scan. It takes a SHARE UPDATE EXCLUSIVE lock: reads and
-- writes go on meanwhile. The column is new, so every row passes.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step VALIDATE CONSTRAINT factory_step_slot_wait_reason_length;
