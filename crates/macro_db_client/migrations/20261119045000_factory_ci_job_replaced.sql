-- The CI jobs a newer push cancelled (crates/factory_records): a `ci` wait
-- keeps, beside the jobs of the head it read last, the runs cancelled on a
-- head a newer push replaced while it waited or since the Run's last wait.
-- Such a job has the same name as the head's own, so the key takes whether
-- it ran on a replaced head.
--
-- The column has a constant default, so no row is rewritten, and every row
-- there is ran on the head its wait read. The key's index is built again:
-- the table is young and small. Give up rather than queue behind a long
-- transaction for the table's lock.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_ci_job
    ADD COLUMN replaced boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT factory_ci_job_replaced CHECK (NOT replaced OR result = 'cancelled'),
    DROP CONSTRAINT factory_ci_job_pkey,
    ADD PRIMARY KEY (run_id, seq, replaced, name);
