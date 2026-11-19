-- Self-improvement failures a person rejected (crates/factory_records): when
-- a person closes a self-improvement Run's pull request unmerged, or cancels
-- its Run, the failures it held are dismissed rather than pending again, so
-- they never start another such Run. They stay with that improvement
-- (`improvement_id`), which keeps them out of the pending index.
--
-- Adding a nullable column rewrites no row. Its check is added NOT VALID and
-- validated after, so no row is scanned under the ACCESS EXCLUSIVE lock. Give
-- up rather than queue behind a long transaction on the table.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_improvement_failure ADD COLUMN dismissed_at timestamptz;

ALTER TABLE factory_improvement_failure
    ADD CONSTRAINT factory_improvement_failure_dismissed CHECK (
        dismissed_at IS NULL OR (improvement_id IS NOT NULL AND addressed_at IS NULL)
    ) NOT VALID;

ALTER TABLE factory_improvement_failure
    VALIDATE CONSTRAINT factory_improvement_failure_dismissed;
