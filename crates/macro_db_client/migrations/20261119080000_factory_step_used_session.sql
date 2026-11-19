-- Every agent session a software factory Step worked in (crates/factory_records).
-- `factory_step.session_id` is only the Step's current session: a Step whose
-- session was lost, or whose Agent could not start, goes on in a new one,
-- which replaces it there. The sessions it left keep what their agents used,
-- so a Run's token budget and Insights count them from here. A Step that
-- went on from another's question shares that Step's session, and both keep
-- it; reads count a session once. A row goes with its Step.
--
-- The table is new. Its foreign key takes a SHARE ROW EXCLUSIVE lock on
-- factory_step until the transaction ends, through the backfill, which
-- reads factory_step once: Steps are few. Give up rather than queue behind a
-- long transaction.
SET LOCAL lock_timeout = '5s';

CREATE TABLE factory_step_used_session (
    run_id uuid NOT NULL,
    seq integer NOT NULL,
    session_id uuid NOT NULL,
    PRIMARY KEY (run_id, seq, session_id),
    FOREIGN KEY (run_id, seq) REFERENCES factory_step (run_id, seq) ON DELETE CASCADE
);

-- Each Step's current session. The sessions Steps left before now were
-- never kept, and are not counted.
INSERT INTO factory_step_used_session (run_id, seq, session_id)
SELECT run_id, seq, session_id
FROM factory_step
WHERE session_id IS NOT NULL
ON CONFLICT DO NOTHING;
