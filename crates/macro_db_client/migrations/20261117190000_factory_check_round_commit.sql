-- The commit a verified round of the factory's checks (crates/factory_records)
-- ran on: the head of the Run's branch on GitHub when the round's first
-- check ran, checked out in a worktree of its own on the agent's machine.
--
-- Takes a lock on factory_check_round until the transaction ends: give up
-- rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

-- New, and null in every row. A verified round records its commit with its
-- first check's run; no deployed code has recorded a run yet, so the check
-- holds for every row.
ALTER TABLE factory_check_round
    ADD COLUMN checked_commit text
        CHECK (checked_commit ~ '^[0-9a-f]{40}([0-9a-f]{24})?$'),
    ADD CONSTRAINT factory_check_round_commit CHECK ((checked_commit IS NULL) = (NOT verified));
