-- A person quarantines a CI job by hand, or releases it, in the software
-- factory's Insights (crates/factory_records). Quarantines are per
-- repository, whoever's team the person administers.
--
-- A quarantine by hand keeps who made it (`quarantined_by`) and covers the
-- whole job, every failure of it, whatever step or test failed. It files no
-- task and counts no passed retries when it is made; a passed rerun of the
-- job extends it, as it extends any quarantine, and quarantines no narrower
-- failure of the job beside it. It lapses as any quarantine does, 30 days
-- after its last passed rerun, or after it was made while none passed.
--
-- A release (`factory_ci_release`) lifts every quarantine of a job in a
-- repository and keeps when: the passed reruns and greens before it no
-- longer prove the job flaky, so only new evidence quarantines it again.
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

-- When a job of a repository (its URL in lower case) was last released:
-- evidence of its flakiness from before then counts for nothing.
CREATE TABLE factory_ci_release (
    repo_url text NOT NULL CHECK (length(repo_url) BETWEEN 1 AND 300),
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 1024),
    released_by text NOT NULL CHECK (octet_length(released_by) BETWEEN 1 AND 1024),
    released_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (repo_url, name)
);
