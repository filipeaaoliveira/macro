-- Validate the dismissal undo check added NOT VALID in 20261120010000, in
-- its own transaction, so the lock that migration took is not held through
-- the scan. It takes a SHARE UPDATE EXCLUSIVE lock: reads and writes go on
-- meanwhile. The columns are new, so every row passes.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_improvement
    VALIDATE CONSTRAINT factory_improvement_dismissal_undone;
