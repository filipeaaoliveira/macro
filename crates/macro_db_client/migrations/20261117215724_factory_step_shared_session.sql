-- A factory Step whose agent asked the Run's owner a question ends, and the
-- Step that goes on with the answer works in the asking Step's agent session
-- (crates/factory_records): the session keeps what the agent did. So a
-- session can have several Steps, one after another, and at most one of them
-- running: the hand back of a session reaches the Step that has not ended.
--
-- This changes factory_step's indexes, so it takes a SHARE lock on it (and
-- the ACCESS EXCLUSIVE lock of dropping the constraint) until the
-- transaction ends. Building the indexes reads the table once each; Steps are
-- few. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_step DROP CONSTRAINT factory_step_session_id_key;

-- At most one running Step works in a session.
CREATE UNIQUE INDEX factory_step_running_session_idx
    ON factory_step (session_id)
    WHERE session_id IS NOT NULL AND ended_at IS NULL;

-- A session's Steps, running or ended, as the hand back and other services
-- find them.
CREATE INDEX factory_step_session_idx
    ON factory_step (session_id)
    WHERE session_id IS NOT NULL;
