-- A retry of a software factory Run that ended `failed`, `infrastructure`
-- or `blocked` (crates/factory_records, `retry_run`): a new Run of the same
-- task, Flow and Factory Definition commit that keeps the Steps the retried
-- Run took before the entry it stopped at, and starts at that entry.
-- `retried_from` is the Run it retries; `branch_run_id` the Run whose branch
-- it works on, the first of a chain of retries, so every retry pushes to
-- and proposes from that one branch; `first_seq` the place its own first
-- Step takes, past the Steps it kept. All three are null for a Run that
-- retries none.
--
-- The Run it retries is kept by id, with no foreign key: a Run whose
-- workflow started is never removed. A Run is retried at most once
-- (`factory_run_retried_from_idx`, built concurrently in
-- 20261123120200_factory_run_retried_from_index.sql).
--
-- Adding nullable columns rewrites no rows. The check is added NOT VALID,
-- and validated in 20261123120100_factory_run_retry_validate.sql. Give up
-- rather than queue behind a long transaction on the Runs.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    ADD COLUMN retried_from uuid,
    ADD COLUMN branch_run_id uuid,
    ADD COLUMN first_seq integer,
    ADD CONSTRAINT factory_run_retry CHECK (
        (retried_from IS NULL) = (branch_run_id IS NULL)
        AND (retried_from IS NULL) = (first_seq IS NULL)
        AND (first_seq IS NULL OR first_seq >= 1)
        AND (retried_from IS NULL OR retried_from <> id)
    ) NOT VALID;
