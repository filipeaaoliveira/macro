-- The Step Terminal of the software factory (crates/factory_records): the
-- Run's owner watches a running agent Step's session and steers it. They
-- send its agent a note, pause it, resume it, or stop it; the Step keeps
-- each of these, and the state the Step's work reads while it waits on its
-- agent.
--
-- The new columns are nullable or have a constant default, so no row is
-- rewritten; the check scans factory_step under its ACCESS EXCLUSIVE lock,
-- which a small table holds briefly. The new table references
-- factory_step, which takes a SHARE ROW EXCLUSIVE lock on it until the
-- transaction ends. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

-- Since when the owner paused the Step, while it is paused; when they asked
-- it to stop; the prompt the owner's latest note or resume sent its
-- session, which the Step's work follows once the turn before it ends,
-- cleared when the factory sends the session a prompt of its own; and
-- whether the Step stopped taking the owner's steering, as its work did
-- once its agent's turn ended with nothing more to follow (a later prompt
-- of the factory's opens it again). All null or false for Steps from
-- before, and for a Step no one steered.
ALTER TABLE factory_step
    ADD COLUMN paused_at timestamptz,
    ADD COLUMN stop_asked_at timestamptz,
    ADD COLUMN steer_action_id uuid,
    ADD COLUMN steer_closed boolean NOT NULL DEFAULT false,
    ADD CONSTRAINT factory_step_steering CHECK (
        session_id IS NOT NULL
        OR (paused_at IS NULL AND stop_asked_at IS NULL AND steer_action_id IS NULL)
    );

-- What the owner did to a running Step, in order (`place`, from 1): a note
-- to its agent, a pause, a resume (with an optional note), or a stop. A
-- note and a resume each sent the session a prompt (`prompt_id`, a
-- UUIDv7 the factory makes); `undelivered` marks one the session refused,
-- which the agent never read. A Step's steering goes with it.
CREATE TABLE factory_step_steer (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    place integer NOT NULL CHECK (place >= 1),
    action text NOT NULL CHECK (action IN ('note', 'pause', 'resume', 'stop')),
    note text CHECK (octet_length(note) BETWEEN 1 AND 4096),
    prompt_id uuid,
    undelivered boolean NOT NULL DEFAULT false,
    at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT factory_step_steer_note CHECK (
        (action = 'note' AND note IS NOT NULL)
        OR action = 'resume'
        OR note IS NULL
    ),
    CONSTRAINT factory_step_steer_prompt CHECK (
        (prompt_id IS NOT NULL) = (action IN ('note', 'resume'))
        AND (NOT undelivered OR prompt_id IS NOT NULL)
    ),
    PRIMARY KEY (run_id, seq, place),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);
