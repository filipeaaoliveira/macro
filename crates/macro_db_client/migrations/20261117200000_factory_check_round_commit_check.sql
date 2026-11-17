-- A verified round of the factory's checks (crates/factory_records) keeps
-- the commit its checks ran on; a round the agent reports keeps none. Only
-- that direction is checked: the constraint that also required every
-- verified round to have a commit is replaced, so no row written before
-- the commit was recorded can ever break it.
--
-- Takes a lock on factory_check_round until the transaction ends: give up
-- rather than queue behind a long transaction on it.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_check_round
    DROP CONSTRAINT factory_check_round_commit,
    ADD CONSTRAINT factory_check_round_commit CHECK (checked_commit IS NULL OR verified) NOT VALID;

ALTER TABLE factory_check_round VALIDATE CONSTRAINT factory_check_round_commit;
