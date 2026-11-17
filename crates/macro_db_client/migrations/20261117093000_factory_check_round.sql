-- The checks rounds of the factory's write and fix Steps
-- (crates/factory_records): after the agent's work, the Step has the agent
-- run the checks the repository declares, in the same session, and sends a
-- failure back to it, a set number of rounds at most.
--
-- The new table references factory_step, and the new column changes it, so
-- this takes locks on factory_step until the transaction ends. Give up
-- rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

-- The prompt of the Step's session the agent works on now, when it is not
-- the first: a checks round's, or the fix of a round that failed. Its agent
-- hands back one of `offered`, which a new prompt replaces, and a stop
-- reads this prompt's progress. Null while the first prompt's turn is the
-- latest. The column is new and null in every row, so its check holds.
ALTER TABLE factory_step
    ADD COLUMN turn_action_id uuid
    CONSTRAINT factory_step_turn CHECK (turn_action_id IS NULL OR session_id IS NOT NULL);

-- One round of a Step's checks: the work it checks, what the agent handed
-- back for it, the checks it runs, and how they ended. A round that failed
-- with rounds left sends the failure back to the agent in its own prompt
-- (`fix_action_id`), whose work the next round checks. A round goes with
-- its Step.
CREATE TABLE factory_check_round (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    round integer NOT NULL CHECK (round BETWEEN 1 AND 10),
    work_output text NOT NULL CHECK (octet_length(work_output) <= 65536),
    -- The checks the round runs, in the repository's order: their names,
    -- and the command of each at the same place.
    checks text[] NOT NULL CHECK (cardinality(checks) >= 1),
    commands text[] NOT NULL,
    -- The repository's checks file as the first round read it from the
    -- run's base branch: every round goes by it, so the agent's branch
    -- cannot change which checks run or how many rounds there are.
    declared text NOT NULL CHECK (octet_length(declared) <= 65536),
    -- The prompt that asks the agent to run them.
    check_action_id uuid NOT NULL,
    started_at timestamptz NOT NULL DEFAULT now(),
    -- passed: every check passed; failed: one did not, or one could not run.
    -- All three are null while the checks run.
    outcome text CHECK (outcome IN ('passed', 'failed')),
    report text CHECK (octet_length(report) <= 65536),
    ended_at timestamptz,
    fix_action_id uuid,
    CONSTRAINT factory_check_round_end CHECK (
        (outcome IS NULL) = (ended_at IS NULL) AND (report IS NULL) = (ended_at IS NULL)
    ),
    CONSTRAINT factory_check_round_commands CHECK (cardinality(commands) = cardinality(checks)),
    CONSTRAINT factory_check_round_fix CHECK (fix_action_id IS NULL OR outcome = 'failed'),
    PRIMARY KEY (run_id, seq, round),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);
