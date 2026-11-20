-- What failed in a CI job finer than its step (crates/factory_records): the
-- first test its check run annotated as failed, by its file and name. CI
-- health calls a failure flaky only when the same test, or the same named
-- step, failed on two or more pull requests; a generic step such as
-- "Run tests" is no such identity.
--
-- The column is new and nullable, so no row is rewritten; its checks pass
-- on every row there is. Give up rather than queue behind a long
-- transaction for the table's lock.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_ci_job
    ADD COLUMN failure text CHECK (octet_length(failure) BETWEEN 1 AND 1024),
    ADD CONSTRAINT factory_ci_job_failure CHECK (
        failure IS NULL OR result IN ('failed', 'setup')
    );
