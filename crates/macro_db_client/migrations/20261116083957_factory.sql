-- The software factory's records (crates/factory_records): each Run of a
-- Flow, the Steps it took, and the Gates they asked people. A Run walks its
-- Flow on Temporal; each Step is one visit of one of the Flow's entries.
--
-- The new tables reference "User" and "Document", so creating them takes a
-- SHARE ROW EXCLUSIVE lock on both until this transaction ends. Give up
-- rather than queue behind a long transaction on either.
SET LOCAL lock_timeout = '5s';

-- A Run belongs to the user who started it: its agent Steps run as them,
-- and it goes with their account. It walks one built-in Flow, by name (null
-- only for Runs recorded before the Flow was kept). It may work on one Macro
-- task, which relinking replaces; deleting the task document unlinks it
-- rather than deleting the Run.
CREATE TABLE factory_run (
    id uuid PRIMARY KEY,
    user_id text NOT NULL REFERENCES "User" (id) ON DELETE CASCADE,
    flow text CHECK (flow ~ '^[a-z0-9_-]{1,64}$'),
    request text NOT NULL CHECK (octet_length(request) <= 65536),
    task_id text REFERENCES "Document" (id) ON DELETE SET NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    -- success: the Flow reached its end; manual: it ended for a person to
    -- take over; blocked: an entry ran out of visits with nowhere to go, or
    -- the Run took its most Steps; failed: a Step failed with nowhere to go,
    -- or the Flow had no way on; cancelled: a person cancelled it. Both are
    -- null while the Run goes.
    outcome text CHECK (outcome IN ('success', 'manual', 'blocked', 'failed', 'cancelled')),
    ended_at timestamptz,
    CONSTRAINT factory_run_end CHECK ((outcome IS NULL) = (ended_at IS NULL))
);

-- Insights read one person's Runs created in a time range, newest first, and
-- stop at a cap. Its leading column also finds a deleted user's Runs without
-- a full scan.
CREATE INDEX factory_run_user_created_idx
    ON factory_run (user_id, created_at DESC, id DESC);

-- A task's Runs are listed for the person who started them, newest first:
-- this index finds one person's Runs on one task in that order and stops at
-- the page's end. Its leading column also finds a deleted task document's
-- Runs.
CREATE INDEX factory_run_task_owner_idx
    ON factory_run (task_id, user_id, created_at DESC, id DESC)
    WHERE task_id IS NOT NULL;

-- A Step belongs to its Run and goes with it. `seq` is its place among the
-- Run's Steps; a reset numbers an entry's visits from 1 again, but never
-- reuses a place, so every visit keeps its own record.
CREATE TABLE factory_step (
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    seq integer NOT NULL CHECK (seq BETWEEN 1 AND 65535),
    entry text NOT NULL CHECK (entry ~ '^[a-z0-9_-]{1,64}$'),
    visit integer NOT NULL CHECK (visit BETWEEN 1 AND 65535),
    started_at timestamptz NOT NULL DEFAULT now(),
    -- The outcome the Flow routes, and what the Step produced for a person
    -- to read. All three are null while the Step runs.
    outcome text CHECK (outcome ~ '^[a-z0-9_-]{1,64}$'),
    output text CHECK (octet_length(output) <= 65536),
    ended_at timestamptz,
    -- An agent Step's Macro agent session and its first prompt's action,
    -- both caller-made UUIDv7s, and the outcomes its agent may hand back.
    -- All null for a Step no agent does.
    session_id uuid UNIQUE,
    action_id uuid,
    offered text[],
    -- The agent's latest hand back; null until it hands one back.
    handed_outcome text,
    handed_output text CHECK (octet_length(handed_output) <= 65536),
    handed_at timestamptz,
    -- When the Step let go of its session: stopped it for good, or deleted
    -- it once its end was recorded. An attempt still running after that,
    -- such as one Temporal timed out, must not open it again under its id.
    session_released_at timestamptz,
    -- When an attempt of the Step last asked for its session to be opened,
    -- and when one first saw it open. A stop writes off a session it can't
    -- delete only when none opened and no prepare can still land.
    prepared_at timestamptz,
    opened_at timestamptz,
    CONSTRAINT factory_step_end CHECK (
        (outcome IS NULL) = (ended_at IS NULL) AND (output IS NULL) = (ended_at IS NULL)
    ),
    CONSTRAINT factory_step_session CHECK (
        (session_id IS NULL) = (action_id IS NULL)
        AND (session_id IS NULL) = (offered IS NULL)
    ),
    CONSTRAINT factory_step_session_released CHECK (
        session_released_at IS NULL OR session_id IS NOT NULL
    ),
    CONSTRAINT factory_step_session_opening CHECK (
        (prepared_at IS NULL OR session_id IS NOT NULL)
        AND (opened_at IS NULL OR session_id IS NOT NULL)
    ),
    CONSTRAINT factory_step_hand_back CHECK (
        (handed_outcome IS NULL) = (handed_at IS NULL)
        AND (handed_output IS NULL) = (handed_at IS NULL)
        AND (handed_at IS NULL OR session_id IS NOT NULL)
        AND (handed_outcome IS NULL OR handed_outcome = ANY (offered))
    ),
    PRIMARY KEY (run_id, seq)
);

-- A Gate is the question a Step's end puts to a person; the Run waits for
-- the answer, which is the outcome its Flow routes. It goes with its Step.
-- Who answered is cleared when their account is deleted; the answer stays.
CREATE TABLE factory_gate (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    question text NOT NULL CHECK (octet_length(question) BETWEEN 1 AND 4096),
    answers text[] NOT NULL CHECK (cardinality(answers) >= 1),
    opened_at timestamptz NOT NULL DEFAULT now(),
    -- All null while the Run waits.
    answer text,
    note text CHECK (octet_length(note) <= 4096),
    user_id text REFERENCES "User" (id) ON DELETE SET NULL,
    answered_at timestamptz,
    CONSTRAINT factory_gate_answer CHECK (
        (answer IS NULL) = (answered_at IS NULL)
        AND (answer IS NOT NULL OR (note IS NULL AND user_id IS NULL))
    ),
    CONSTRAINT factory_gate_answer_offered CHECK (answer IS NULL OR answer = ANY (answers)),
    PRIMARY KEY (run_id, seq),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);

-- Deleting a user finds their answers without scanning every Gate.
CREATE INDEX factory_gate_user_id_idx ON factory_gate (user_id) WHERE user_id IS NOT NULL;
