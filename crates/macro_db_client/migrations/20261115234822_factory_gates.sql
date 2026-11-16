-- Gates and cancelled Runs of the software factory (crates/factory_records).
-- A Gate is a question a Step's end puts to a person; the Run waits for the
-- answer, which is the outcome its Flow routes. Both tables are the factory's
-- own and only a few days old, so the constraint swap locks nothing for long.

ALTER TABLE factory_run DROP CONSTRAINT factory_run_outcome_check;
ALTER TABLE factory_run ADD CONSTRAINT factory_run_outcome_check
    CHECK (outcome IN ('success', 'manual', 'blocked', 'failed', 'cancelled'));

-- A Gate belongs to its Step and goes with it. Who answered is cleared when
-- their account is deleted; the answer stays with the Run.
CREATE TABLE factory_gate (
    run_id uuid NOT NULL,
    entry text NOT NULL,
    visit integer NOT NULL,
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
    PRIMARY KEY (run_id, entry, visit),
    FOREIGN KEY (run_id, entry, visit)
        REFERENCES factory_step (run_id, entry, visit) ON DELETE CASCADE
);

-- Deleting a user finds their answers without scanning every Gate.
CREATE INDEX factory_gate_user_id_idx ON factory_gate (user_id) WHERE user_id IS NOT NULL;
