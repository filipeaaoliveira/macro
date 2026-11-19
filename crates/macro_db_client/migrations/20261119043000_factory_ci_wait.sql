-- How each wait of the factory's `ci` Step on a pull request's checks ended
-- (crates/factory_records), and each CI job it saw finish, for CI health in
-- insights: a job's red rate, and whether a failure was the pull request's,
-- the CI's setup or a flaky job, or a run a newer push cancelled.
--
-- Both tables are new and empty, so no row is rewritten or checked. Their
-- foreign keys take a SHARE ROW EXCLUSIVE lock on factory_step until the
-- transaction ends; give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

-- One wait, kept once its `ci` Step reads its verdict: green, red or
-- timeout. A retry of the Step keeps the first. A wait goes with its Step.
CREATE TABLE factory_ci_wait (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    -- The commit whose checks the wait read last.
    head_sha text NOT NULL CHECK (head_sha ~ '^[0-9a-f]{40}([0-9a-f]{24})?$'),
    verdict text NOT NULL CHECK (verdict IN ('green', 'red', 'timeout')),
    recorded_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, seq),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);

-- Each CI job (a check run, or a commit status) the wait saw finish on that
-- commit, by name. passed: it passed, or was skipped or neutral; failed: its
-- own work failed; setup: it failed setting up, before its work ran;
-- cancelled: it was cancelled, as when a newer push replaced its run. A
-- failed job keeps the first of its steps that failed, when it was read.
CREATE TABLE factory_ci_job (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    name text NOT NULL CHECK (octet_length(name) BETWEEN 1 AND 1024),
    result text NOT NULL CHECK (result IN ('passed', 'failed', 'setup', 'cancelled')),
    failed_step text CHECK (octet_length(failed_step) BETWEEN 1 AND 1024),
    CONSTRAINT factory_ci_job_failed_step CHECK (
        failed_step IS NULL OR result IN ('failed', 'setup')
    ),
    PRIMARY KEY (run_id, seq, name),
    FOREIGN KEY (run_id, seq) REFERENCES factory_ci_wait (run_id, seq) ON DELETE CASCADE
);
