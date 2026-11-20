-- Whether the last cancel sent to a factory Run before it ended came from
-- its owner in person (crates/factory_records). Self-improvement reads it: a
-- self-improvement Run a person cancelled is their rejection, which dismisses
-- its failures; one an agent, a bot under a Standing Role or anything else
-- cancelled only did not finish, and its failures are pending again. Runs
-- cancelled before this read as not in person, which never dismisses.
--
-- A column with a constant default rewrites no row. Give up rather than
-- queue behind a long transaction on the table.
SET LOCAL lock_timeout = '5s';

ALTER TABLE factory_run
    ADD COLUMN cancelled_in_person boolean NOT NULL DEFAULT false;
