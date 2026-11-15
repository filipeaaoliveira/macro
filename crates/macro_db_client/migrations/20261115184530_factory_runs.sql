-- The software factory's Runs and their Steps (crates/factory_records). A Run
-- walks a Flow on Temporal; each Step is one visit of one of the Flow's
-- entries. Both tables are new, so nothing here locks an existing one.

CREATE TABLE factory_run (
    id uuid PRIMARY KEY,
    request text NOT NULL CHECK (octet_length(request) <= 65536),
    created_at timestamptz NOT NULL DEFAULT now(),
    -- success: the Flow reached its end; manual: it ended for a person to
    -- take over; blocked: an entry ran out of visits with nowhere to go;
    -- failed: a Step failed with nowhere to go, or the Flow had no way on.
    -- Both are null while the Run goes.
    outcome text CHECK (outcome IN ('success', 'manual', 'blocked', 'failed')),
    ended_at timestamptz,
    CONSTRAINT factory_run_end CHECK ((outcome IS NULL) = (ended_at IS NULL))
);

-- A Step belongs to its Run and goes with it.
CREATE TABLE factory_step (
    run_id uuid NOT NULL REFERENCES factory_run(id) ON DELETE CASCADE,
    entry text NOT NULL CHECK (entry ~ '^[a-z0-9_-]{1,64}$'),
    visit integer NOT NULL CHECK (visit BETWEEN 1 AND 65535),
    started_at timestamptz NOT NULL DEFAULT now(),
    -- The outcome the Flow routes, and what the Step produced for a person to
    -- read. All three are null while the Step runs.
    outcome text CHECK (outcome ~ '^[a-z0-9_-]{1,64}$'),
    output text CHECK (octet_length(output) <= 65536),
    ended_at timestamptz,
    CONSTRAINT factory_step_end CHECK (
        (outcome IS NULL) = (ended_at IS NULL) AND (output IS NULL) = (ended_at IS NULL)
    ),
    PRIMARY KEY (run_id, entry, visit)
);
