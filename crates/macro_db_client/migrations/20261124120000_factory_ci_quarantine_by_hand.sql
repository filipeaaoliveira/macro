-- A person quarantines a CI job by hand, or lifts it, in the software
-- factory's Insights (crates/factory_records). A quarantine by hand names
-- the job alone, with no step or test, so it covers every failure of the
-- job; it keeps who made it (`quarantined_by`), files no task, and counts
-- no passed retries until a rerun of the job passes. It lapses as any
-- quarantine does, 30 days after its last passed rerun, or after it was
-- made while none passed.
--
-- The new column is nullable, so no row is rewritten. Swapping the check
-- scans this small table under its ACCESS EXCLUSIVE lock, briefly. Give up
-- rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_ci_quarantine
    ADD COLUMN quarantined_by text CHECK (octet_length(quarantined_by) BETWEEN 1 AND 1024),
    DROP CONSTRAINT factory_ci_quarantine_passed_retries_check,
    ADD CONSTRAINT factory_ci_quarantine_passed_retries_check CHECK (
        passed_retries > 0 OR (quarantined_by IS NOT NULL AND passed_retries = 0)
    );
