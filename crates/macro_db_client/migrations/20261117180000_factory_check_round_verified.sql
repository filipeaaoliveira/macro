-- Whether a round of the factory's checks (crates/factory_records) was run
-- by the engine itself, which read each check's exit code, or by the agent,
-- which reported how they went; and, for a verified round, how each check
-- ran.
--
-- Takes a lock on factory_check_round until the transaction ends: give up
-- rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

-- Both columns are new: every round recorded so far was reported by its
-- agent, which the default says, and has no results. A constant default
-- changes no row, and the new check holds for every one of them.
ALTER TABLE factory_check_round
    ADD COLUMN verified boolean NOT NULL DEFAULT false,
    -- Each check the engine ran so far, as it ran: its name, exit code and
    -- the end of its output, [{"name", "exitCode", "output"}], in the
    -- round's order. A verified round has results; a reported one none.
    ADD COLUMN results jsonb,
    ADD CONSTRAINT factory_check_round_results CHECK (
        (results IS NULL) = (NOT verified)
        AND octet_length(results::text) <= 524288
    );
