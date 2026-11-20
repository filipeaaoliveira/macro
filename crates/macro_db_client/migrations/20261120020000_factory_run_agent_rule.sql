-- The Agent Rule of its Factory Definition that chose the Agent a Run's
-- Steps of one Role start with (crates/factory_records): the rule matched
-- the Run's task tags or its repository when the Run started. A Run keeps
-- at most one per Role, and its Steps read it from here, so the choice is
-- made once and a Run under way keeps it, whatever its task's tags or its
-- team's Definition say later.
--
-- The new table references factory_run, so creating it takes a SHARE ROW
-- EXCLUSIVE lock on factory_run until this transaction ends. No existing
-- row is rewritten. Give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

CREATE TABLE factory_run_agent_rule (
    run_id uuid NOT NULL REFERENCES factory_run (id) ON DELETE CASCADE,
    role text NOT NULL CHECK (role ~ '^[a-z0-9_-]{1,64}$'),
    rule text NOT NULL CHECK (rule ~ '^[a-z0-9_-]{1,64}$'),
    agent text NOT NULL CHECK (agent ~ '^[a-z0-9_-]{1,64}$'),
    PRIMARY KEY (run_id, role)
);
