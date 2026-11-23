-- Validate the Runs' retry check added NOT VALID in 20261123120000, in its
-- own transaction, so the lock that migration took is not held through the
-- scan. It takes a SHARE UPDATE EXCLUSIVE lock: reads and writes go on
-- meanwhile. The columns are new, so every row passes.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run VALIDATE CONSTRAINT factory_run_retry;
