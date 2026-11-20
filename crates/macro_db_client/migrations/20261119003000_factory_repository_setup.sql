-- A repository's setup for the factory (crates/factory_records): the
-- commands in `.macro/setup.json` on a Run's base that prepare a fresh
-- checkout. A Step keeps how many of them passed in its agent session, so
-- a retry in the same session skips them, and a round of checks keeps the
-- setup file it read, which every round runs in its worktree first.
--
-- Takes locks on factory_step and factory_check_round until the
-- transaction ends: give up rather than queue behind a long transaction.
SET LOCAL lock_timeout = '5s';

-- New, and null or zero in every row: no Step ran a setup yet. A constant
-- default changes no row, and the checks hold for every one of them.
ALTER TABLE factory_step
    ADD COLUMN setup_session_id uuid,
    ADD COLUMN setup_passed smallint NOT NULL DEFAULT 0 CHECK (setup_passed BETWEEN 0 AND 16);

ALTER TABLE factory_check_round
    ADD COLUMN setup text CHECK (octet_length(setup) <= 16384);
