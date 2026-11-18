-- Standing acts in the software factory (crates/factory_records): what an
-- agent working for a Run's owner did to the Run from its code under a
-- Standing Role of the Run's Factory Definition, with the bot whose
-- session did it, the Definition's Agent for that bot and the Standing
-- Role that allowed it.
--
-- A Run keeps one start, one cancel and one answer per Gate (`seq`, the
-- Step that asked); each link to a task is kept. A new table: no existing
-- row is read or rewritten.
CREATE TABLE factory_standing_act (
    id            uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    run_id        uuid        NOT NULL REFERENCES factory_run(id) ON DELETE CASCADE,
    act           text        NOT NULL
                              CHECK (act IN ('start', 'cancel', 'link', 'answer_plan', 'answer_blockers')),
    seq           integer,
    bot_id        uuid        NOT NULL,
    agent         text        NOT NULL,
    standing_role text        NOT NULL,
    session_id    uuid        NOT NULL,
    at            timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT factory_standing_act_seq CHECK (
        (seq IS NOT NULL) = (act IN ('answer_plan', 'answer_blockers'))
    )
);

CREATE INDEX factory_standing_act_run_idx ON factory_standing_act (run_id, at);

CREATE UNIQUE INDEX factory_standing_act_once_idx
    ON factory_standing_act (run_id, act, (coalesce(seq, -1)))
    WHERE act <> 'link';
