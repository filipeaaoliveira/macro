-- Break records (crates/factory_records): the evidence that a new test
-- fails when the code it covers breaks. A write or fix Step's agent reports
-- one break per test it adds: a file, the text to replace in it and what
-- replaces it, and the command that runs the test. The Step's checks round
-- runs each break on the commit whose checks passed (the test without the
-- break, then with it) and records how it went. The reviewers read the
-- records, and the Run's page lists them. A Step records each test's break
-- once (an interrupted run's record is replaced by the next run's), and
-- its records go with the Step.
--
-- The new table references factory_step, which takes a SHARE ROW EXCLUSIVE
-- lock on it until this transaction ends. Give up rather than queue behind
-- a long transaction.
SET LOCAL lock_timeout = '5s';

CREATE TABLE factory_break_record (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    test text NOT NULL CHECK (octet_length(test) BETWEEN 1 AND 1024),
    file text NOT NULL CHECK (octet_length(file) <= 1024),
    old_text text NOT NULL CHECK (octet_length(old_text) <= 1024),
    new_text text NOT NULL CHECK (octet_length(new_text) <= 1024),
    command text NOT NULL CHECK (octet_length(command) <= 1024),
    -- verified: the test passed without the break and failed with it;
    -- not_reproduced: it passed with it too; not_run: it failed without
    -- it; not_applied: the file does not hold the text to replace;
    -- unreadable: the agent's line does not read as a break; dropped: no
    -- time was left, or too many were reported; unchecked: the factory ran
    -- nothing where the agent works; interrupted: the run ended before it
    -- said how the test went, and a later run replaces the record.
    status text NOT NULL CHECK (
        status IN (
            'verified', 'not_reproduced', 'not_run', 'not_applied', 'unreadable', 'dropped',
            'unchecked', 'interrupted'
        )
    ),
    checked_commit text CHECK (checked_commit ~ '^[0-9a-f]{40}([0-9a-f]{24})?$'),
    output text NOT NULL CHECK (octet_length(output) <= 4096),
    recorded_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, seq, test),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);
